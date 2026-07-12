#[allow(dead_code)]
mod lowering_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use lowering_support::{assert_or_update_fixture, repository_root};
use sv_to_sexpr::analyze::{ModuleCatalog, analyze_design_with_catalog_and_generate_mode};
use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{Cell, CellItem, Expr, ValueOperator};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::collect_sv_files;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FailureCategory {
    TransistorRelated,
    KeeperUser,
    UnsupportedTimingFactor,
}

impl FailureCategory {
    fn label(self) -> &'static str {
        match self {
            Self::TransistorRelated => "transistor-related",
            Self::KeeperUser => "keeper-user",
            Self::UnsupportedTimingFactor => "unsupported-timing-factor",
        }
    }
}

#[derive(Debug)]
struct FailedFile {
    category: FailureCategory,
    path: String,
    diagnostic: Diagnostic,
}

#[derive(Default)]
struct AuditTotals {
    processed: usize,
    succeeded: usize,
    failed: usize,
    invalid_successful_cells: usize,
    nested_value_expressions: usize,
    empty_value_operands: usize,
    timing_validation_failures: usize,
    nondeterministic_results: usize,
    absolute_path_leaks: usize,
    dependency_order_failures: usize,
    assignments: usize,
    atom_value_assignments: usize,
    temporary_assignments: usize,
    repeated_target_assignments: usize,
    delayed_assignments: usize,
    nested_timing_delay_assignments: usize,
    cells_with_registers: usize,
    registers: usize,
    operator_counts: BTreeMap<String, usize>,
}

const TRANSISTOR_FAILURES: &[&str] = &[
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/idu_bit123456.sv",
    "sv-cells/sm83/cells/irq_prio_bit0.sv",
    "sv-cells/sm83/cells/irq_prio_bit1.sv",
    "sv-cells/sm83/cells/irq_prio_bit2.sv",
    "sv-cells/sm83/cells/irq_prio_bit3.sv",
    "sv-cells/sm83/cells/irq_prio_bit4.sv",
    "sv-cells/sm83/cells/irq_prio_bit5.sv",
    "sv-cells/sm83/cells/irq_prio_bit6.sv",
];
const KEEPER_FAILURES: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/mux.sv",
    "sv-cells/dmg_cpu_b/cells/muxi.sv",
    "sv-cells/dmg_cpu_b/cells/pad_xtal.sv",
    "sv-cells/sm83/cells/idu_bit0.sv",
    "sv-cells/sm83/cells/reg_wz_out.sv",
];
const TIMING_FACTOR_FAILURES: &[&str] = &[];

#[test]
fn full_corpus_lowering_baseline_is_deterministic_flat_and_explicit() {
    let root = repository_root();
    let corpus_root = root.join("sv-cells");
    let mut paths = collect_sv_files(&corpus_root)
        .unwrap()
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap()
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), 206);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let mut totals = AuditTotals::default();
    let mut failures = Vec::new();
    let absolute_root = root.to_string_lossy().to_string();
    let designs = paths
        .iter()
        .map(|logical_path| {
            let input = fs::read_to_string(root.join(logical_path)).unwrap();
            (
                logical_path.clone(),
                parse_file(Path::new(logical_path), &input).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let catalog =
        ModuleCatalog::from_designs(&designs.values().cloned().collect::<Vec<_>>()).unwrap();

    for logical_path in &paths {
        totals.processed += 1;
        let design = &designs[logical_path];
        let first =
            lower_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful);
        let second =
            lower_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                totals.succeeded += 1;
                if first != second {
                    totals.nondeterministic_results += 1;
                }
                let first_render = render_cell(&first.cell);
                let second_render = render_cell(&second.cell);
                if first_render != second_render {
                    totals.nondeterministic_results += 1;
                }
                if first_render.contains(&absolute_root) {
                    totals.absolute_path_leaks += 1;
                }

                let analysis = analyze_design_with_catalog_and_generate_mode(
                    design,
                    &catalog,
                    GenerateMode::Delayful,
                )
                .unwrap();
                let source_names = analysis.modules[0]
                    .symbols
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                audit_success(&first.cell, &source_names, &mut totals);
            }
            (Err(first), Err(second)) => {
                totals.failed += 1;
                if first != second {
                    totals.nondeterministic_results += 1;
                }
                assert_eq!(
                    first.span.path,
                    Path::new(logical_path),
                    "failure leaked a non-logical path"
                );
                let category = classify_failure(logical_path, &first);
                failures.push(FailedFile {
                    category,
                    path: logical_path.clone(),
                    diagnostic: first,
                });
            }
            (first, second) => panic!(
                "nondeterministic success/failure disposition for {logical_path}: first={first:?} second={second:?}"
            ),
        }
    }

    failures.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then_with(|| left.path.cmp(&right.path))
    });
    assert_exact_baseline(&totals, &failures);
    let summary = render_summary(&totals, &failures);
    assert!(!summary.contains(&absolute_root));
    assert!(summary.contains("processed=206 succeeded=192 failed=14"));
    assert!(summary.contains("invalid_successful_cells=0"));
    assert!(summary.contains("nested_value_expressions=0"));
    assert!(summary.contains("timing_validation_failures=0"));
    assert!(summary.contains("nondeterministic_results=0"));
    for category in [
        FailureCategory::TransistorRelated,
        FailureCategory::KeeperUser,
        FailureCategory::UnsupportedTimingFactor,
    ] {
        assert!(summary.contains(&format!("{} files=", category.label())));
    }
    assert_or_update_fixture("corpus_summary", "lower", &summary);
}

fn audit_success(cell: &Cell, source_names: &BTreeSet<String>, totals: &mut AuditTotals) {
    if cell.validate().is_err() {
        totals.invalid_successful_cells += 1;
    }
    if !cell.registers.is_empty() {
        totals.cells_with_registers += 1;
    }
    totals.registers += cell.registers.len();

    let assignments = cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    totals.assignments += assignments.len();

    let generated_temps = assignments
        .iter()
        .filter(|assignment| {
            temp_index(&assignment.target).is_some() && !source_names.contains(&assignment.target)
        })
        .map(|assignment| assignment.target.clone())
        .collect::<BTreeSet<_>>();
    totals.temporary_assignments += generated_temps.len();

    let mut seen_targets = BTreeSet::new();
    let mut available_temps = BTreeSet::new();
    for assignment in assignments {
        if !seen_targets.insert(assignment.target.clone()) {
            totals.repeated_target_assignments += 1;
        }
        match &assignment.expr {
            Expr::Atom(atom) => {
                totals.atom_value_assignments += 1;
                if atom.is_empty() {
                    totals.empty_value_operands += 1;
                }
            }
            Expr::List(items) => {
                let Some((head, operands)) = items.split_first() else {
                    totals.nested_value_expressions += 1;
                    continue;
                };
                let Expr::Atom(head) = head else {
                    totals.nested_value_expressions += 1;
                    continue;
                };
                let operator = ValueOperator::parse(head)
                    .unwrap_or_else(|| panic!("uncontracted value operator `{head}`"));
                *totals
                    .operator_counts
                    .entry(operator.as_str().to_string())
                    .or_default() += 1;
                for operand in operands {
                    match operand {
                        Expr::Atom(atom) => {
                            if atom.is_empty() {
                                totals.empty_value_operands += 1;
                            }
                            if generated_temps.contains(atom) && !available_temps.contains(atom) {
                                totals.dependency_order_failures += 1;
                            }
                        }
                        Expr::List(_) => totals.nested_value_expressions += 1,
                    }
                }
            }
        }

        if assignment.delay.validate_timing("corpus audit").is_err() {
            totals.timing_validation_failures += 1;
        }
        if assignment.delay != Expr::atom("0") {
            totals.delayed_assignments += 1;
        }
        if timing_has_nested_operation(&assignment.delay) {
            totals.nested_timing_delay_assignments += 1;
        }
        if generated_temps.contains(&assignment.target) {
            available_temps.insert(assignment.target.clone());
        }
    }
}

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

fn timing_has_nested_operation(expr: &Expr) -> bool {
    match expr {
        Expr::Atom(_) => false,
        Expr::List(items) => items.iter().skip(1).any(|operand| {
            matches!(operand, Expr::List(_)) || timing_has_nested_operation(operand)
        }),
    }
}

fn classify_failure(path: &str, diagnostic: &Diagnostic) -> FailureCategory {
    let membership_count = [
        TRANSISTOR_FAILURES.contains(&path),
        KEEPER_FAILURES.contains(&path),
        TIMING_FACTOR_FAILURES.contains(&path),
    ]
    .into_iter()
    .filter(|matches| *matches)
    .count();
    assert_eq!(
        membership_count, 1,
        "lowering failure must match exactly one explicit category: {path}: {diagnostic}"
    );

    let (category, expected_message) = if TRANSISTOR_FAILURES.contains(&path) {
        (FailureCategory::TransistorRelated, "unsupported primitive")
    } else if KEEPER_FAILURES.contains(&path) {
        (FailureCategory::KeeperUser, "unsupported item for lowering")
    } else if TIMING_FACTOR_FAILURES.contains(&path) {
        (
            FailureCategory::UnsupportedTimingFactor,
            "unsupported timing factor",
        )
    } else {
        panic!("uncategorized lowering failure {path}: {diagnostic}");
    };
    assert!(
        diagnostic.message.starts_with(expected_message),
        "unexpected diagnostic category for {path}: {diagnostic}"
    );
    category
}

fn assert_exact_baseline(totals: &AuditTotals, failures: &[FailedFile]) {
    assert_eq!(totals.processed, 206);
    assert_eq!(totals.succeeded, 192);
    assert_eq!(totals.failed, 14);
    assert_eq!(failures.len(), 14);
    assert_eq!(totals.invalid_successful_cells, 0);
    assert_eq!(totals.nested_value_expressions, 0);
    assert_eq!(totals.empty_value_operands, 0);
    assert_eq!(totals.timing_validation_failures, 0);
    assert_eq!(totals.nondeterministic_results, 0);
    assert_eq!(totals.absolute_path_leaks, 0);
    assert_eq!(totals.dependency_order_failures, 0);
    assert_eq!(totals.assignments, 1693);
    assert_eq!(totals.atom_value_assignments, 17);
    assert_eq!(totals.temporary_assignments, 1046);
    assert_eq!(totals.repeated_target_assignments, 51);
    assert_eq!(totals.delayed_assignments, 588);
    assert_eq!(totals.nested_timing_delay_assignments, 588);
    assert_eq!(totals.cells_with_registers, 26);
    assert_eq!(totals.registers, 47);
    assert_eq!(
        totals.operator_counts,
        BTreeMap::from([
            ("and".to_string(), 646),
            ("bufif0".to_string(), 2),
            ("bufif0-strength".to_string(), 74),
            ("bufif1".to_string(), 10),
            ("bufif1-strength".to_string(), 293),
            ("caseeq".to_string(), 4),
            ("drive-strength".to_string(), 5),
            ("mux".to_string(), 54),
            ("nand".to_string(), 27),
            ("nor".to_string(), 28),
            ("not".to_string(), 232),
            ("or".to_string(), 292),
            ("xnor".to_string(), 1),
            ("xor".to_string(), 8),
        ])
    );

    for (category, expected_paths) in [
        (FailureCategory::TransistorRelated, TRANSISTOR_FAILURES),
        (FailureCategory::KeeperUser, KEEPER_FAILURES),
        (
            FailureCategory::UnsupportedTimingFactor,
            TIMING_FACTOR_FAILURES,
        ),
    ] {
        assert_eq!(
            failures
                .iter()
                .filter(|failure| failure.category == category)
                .map(|failure| failure.path.as_str())
                .collect::<Vec<_>>(),
            expected_paths,
            "{} failure set changed",
            category.label()
        );
    }
}

fn render_summary(totals: &AuditTotals, failures: &[FailedFile]) -> String {
    let mut output = String::new();
    writeln!(&mut output, "lowering corpus audit").unwrap();
    writeln!(
        &mut output,
        "processed={} succeeded={} failed={}",
        totals.processed, totals.succeeded, totals.failed
    )
    .unwrap();
    writeln!(&mut output, "invariants:").unwrap();
    for (name, value) in [
        ("invalid_successful_cells", totals.invalid_successful_cells),
        ("nested_value_expressions", totals.nested_value_expressions),
        ("empty_value_operands", totals.empty_value_operands),
        (
            "timing_validation_failures",
            totals.timing_validation_failures,
        ),
        ("nondeterministic_results", totals.nondeterministic_results),
        ("absolute_path_leaks", totals.absolute_path_leaks),
        (
            "dependency_order_failures",
            totals.dependency_order_failures,
        ),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "successful-cell-capabilities:").unwrap();
    for (name, value) in [
        ("assignments", totals.assignments),
        ("atom_value_assignments", totals.atom_value_assignments),
        ("temporary_assignments", totals.temporary_assignments),
        (
            "repeated_target_assignments",
            totals.repeated_target_assignments,
        ),
        ("delayed_assignments", totals.delayed_assignments),
        (
            "nested_timing_delay_assignments",
            totals.nested_timing_delay_assignments,
        ),
        ("cells_with_registers", totals.cells_with_registers),
        ("registers", totals.registers),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "value-operators:").unwrap();
    for (operator, count) in &totals.operator_counts {
        writeln!(&mut output, "  {operator}={count}").unwrap();
    }
    writeln!(&mut output, "failure-categories:").unwrap();
    for category in [
        FailureCategory::TransistorRelated,
        FailureCategory::KeeperUser,
        FailureCategory::UnsupportedTimingFactor,
    ] {
        let category_failures = failures
            .iter()
            .filter(|failure| failure.category == category)
            .collect::<Vec<_>>();
        writeln!(
            &mut output,
            "  {} files={}",
            category.label(),
            category_failures.len()
        )
        .unwrap();
        for failure in category_failures {
            writeln!(
                &mut output,
                "    {}:{}:{}: {}",
                failure.path,
                failure.diagnostic.span.line,
                failure.diagnostic.span.column,
                failure.diagnostic.message
            )
            .unwrap();
        }
    }
    output
}
