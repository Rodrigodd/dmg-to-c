#[allow(dead_code)]
mod analysis_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use analysis_support::corpus;
use sv_to_sexpr::analyze::{DriverSource, TargetMilestone};
use sv_to_sexpr::ast::{BinaryOp, ConstKind, Expr as SvExpr, ExprKind, ItemKind, UnaryOp};
use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, DelayTuple, Expr, ValueOperator};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;
use sv_to_sexpr::serialize::{render_delay_tuple, render_expr};

const TRANSISTOR_FILES: &[&str] = &[
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/idu_bit0.sv",
    "sv-cells/sm83/cells/idu_bit123456.sv",
    "sv-cells/sm83/cells/irq_prio_bit0.sv",
    "sv-cells/sm83/cells/irq_prio_bit1.sv",
    "sv-cells/sm83/cells/irq_prio_bit2.sv",
    "sv-cells/sm83/cells/irq_prio_bit3.sv",
    "sv-cells/sm83/cells/irq_prio_bit4.sv",
    "sv-cells/sm83/cells/irq_prio_bit5.sv",
    "sv-cells/sm83/cells/irq_prio_bit6.sv",
];

#[derive(Default)]
struct ModeTotals {
    succeeded: usize,
    warnings: usize,
    ignores: usize,
    assignments: usize,
    temporaries: usize,
    register_cells: usize,
    registers: usize,
    transistor_files: BTreeSet<String>,
    transistor_counts: BTreeMap<String, usize>,
    rows: Vec<String>,
}

#[test]
fn dual_mode_transistor_corpus_is_exact_direct_flat_and_fully_lowered() {
    let corpus = corpus();
    assert_eq!(corpus.designs.len(), 206);

    let delayful = audit_mode(corpus, GenerateMode::Delayful);
    let nodelay = audit_mode(corpus, GenerateMode::Nodelay);
    assert_eq!(delayful.rows, nodelay.rows);
    assert_eq!(delayful.transistor_files, nodelay.transistor_files);
    assert_eq!(delayful.transistor_counts, nodelay.transistor_counts);
    assert_exact_mode(&delayful, GenerateMode::Delayful);
    assert_exact_mode(&nodelay, GenerateMode::Nodelay);

    let mut summary = String::from("transistor corpus audit\nfiles=206\n");
    writeln!(
        &mut summary,
        "source files={} calls={} nmos={} pmos={} rnmos={}",
        delayful.transistor_files.len(),
        delayful.transistor_counts.values().sum::<usize>(),
        delayful.transistor_counts["nmos"],
        delayful.transistor_counts["pmos"],
        delayful.transistor_counts["rnmos"]
    )
    .unwrap();
    writeln!(
        &mut summary,
        "source-file-set=[{}]",
        delayful
            .transistor_files
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    )
    .unwrap();
    writeln!(
        &mut summary,
        "mode=delayful succeeded={} failed=0 warnings={} intentional-ignores={} assignments={} temps={} register-cells={} registers={}",
        delayful.succeeded,
        delayful.warnings,
        delayful.ignores,
        delayful.assignments,
        delayful.temporaries,
        delayful.register_cells,
        delayful.registers
    )
    .unwrap();
    writeln!(
        &mut summary,
        "mode=nodelay succeeded={} failed=0 warnings={} intentional-ignores={} assignments={} temps={} register-cells={} registers={}",
        nodelay.succeeded,
        nodelay.warnings,
        nodelay.ignores,
        nodelay.assignments,
        nodelay.temporaries,
        nodelay.register_cells,
        nodelay.registers
    )
    .unwrap();
    writeln!(&mut summary, "calls:").unwrap();
    for row in &delayful.rows {
        writeln!(&mut summary, "  {row}").unwrap();
    }
    assert_or_update_fixture(&summary);
}

fn audit_mode(corpus: &analysis_support::CorpusAnalysis, mode: GenerateMode) -> ModeTotals {
    let mut totals = ModeTotals::default();
    for (path, design) in &corpus.designs {
        let analysis = sv_to_sexpr::analyze::analyze_design_with_catalog_and_generate_mode(
            design,
            &corpus.catalog,
            mode,
        )
        .unwrap();
        assert!(analysis.requirements.iter().all(|requirement| {
            requirement.capability_id != "primitive.transistor"
                && requirement.milestone != TargetMilestone::M11Transistors
        }));
        let lowered =
            lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode).unwrap();
        totals.succeeded += 1;
        lowered.cell.validate().unwrap();
        totals.warnings += lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
            .count();
        totals.ignores += lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
            .count();
        assert!(lowered.diagnostics.iter().all(|diagnostic| {
            let message = diagnostic.message.to_ascii_lowercase();
            !message.contains("transistor")
                && !message.contains("unsupported primitive nmos")
                && !message.contains("unsupported primitive pmos")
                && !message.contains("unsupported primitive rnmos")
        }));

        let assignments = lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some(assignment),
                CellItem::Blank | CellItem::Comment(_) => None,
            })
            .collect::<Vec<_>>();
        totals.assignments += assignments.len();
        let source_names = analysis.modules[0]
            .symbols
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        totals.temporaries += assignments
            .iter()
            .filter(|assignment| !source_names.contains(assignment.target.as_str()))
            .count();
        if !lowered.cell.registers.is_empty() {
            totals.register_cells += 1;
        }
        totals.registers += lowered.cell.registers.len();

        let calls = design
            .first_module()
            .unwrap()
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Primitive(call) if is_transistor_name(&call.name) => Some(call),
                _ => None,
            })
            .collect::<Vec<_>>();
        let analyzed_calls = analysis.modules[0]
            .primitive_calls
            .iter()
            .filter(|call| is_transistor_name(&call.name))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), analyzed_calls.len(), "{path}");
        if calls.is_empty() {
            assert!(!TRANSISTOR_FILES.contains(&path.as_str()));
            continue;
        }
        assert!(TRANSISTOR_FILES.contains(&path.as_str()));
        totals.transistor_files.insert(path.clone());

        let roots = assignments
            .iter()
            .enumerate()
            .filter_map(|(index, assignment)| {
                transistor_operation(&assignment.expr)
                    .map(|(operator, operands)| (index, *assignment, operator, operands))
            })
            .collect::<Vec<_>>();
        assert_eq!(roots.len(), calls.len(), "{path}");
        let drivers = analysis.modules[0]
            .drivers
            .iter()
            .filter(|driver| {
                matches!(
                    &driver.source,
                    DriverSource::Primitive { name } if is_transistor_name(name)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(drivers.len(), calls.len(), "{path}");
        let transistor_driver_counts =
            drivers
                .iter()
                .fold(BTreeMap::<&str, usize>::new(), |mut counts, driver| {
                    *counts.entry(driver.target.as_str()).or_default() += 1;
                    counts
                });
        let emitted_transistor_counts = roots.iter().fold(
            BTreeMap::<&str, usize>::new(),
            |mut counts, (_, assignment, _, _)| {
                *counts.entry(assignment.target.as_str()).or_default() += 1;
                counts
            },
        );
        assert_eq!(
            emitted_transistor_counts, transistor_driver_counts,
            "transistor driver/assignment parity for {path} in {mode:?} mode"
        );

        let temp_exprs = assignments
            .iter()
            .filter(|assignment| !source_names.contains(assignment.target.as_str()))
            .map(|assignment| (assignment.target.as_str(), &assignment.expr))
            .collect::<BTreeMap<_, _>>();
        for (((call, analyzed), driver), (assignment_index, emitted, operator, operands)) in
            calls.iter().zip(analyzed_calls).zip(drivers).zip(roots)
        {
            let expected_operator = operator_for_name(&call.name);
            assert_eq!(operator, expected_operator, "{path}");
            assert_eq!(operands.len(), 2, "{path}");
            assert!(
                operands
                    .iter()
                    .all(|operand| matches!(operand, Expr::Atom(_)))
            );
            assert_eq!(call.args.len(), 3, "{path}");
            assert!(call.args.iter().all(Option::is_some), "{path}");
            assert!(call.strength.is_none(), "{path}");
            assert_eq!(call.span, analyzed.span, "{path}");
            assert_eq!(driver.source_order, analyzed.source_order, "{path}");
            assert_eq!(
                driver.target,
                analyzed.args[0].as_deref().unwrap(),
                "{path}"
            );
            assert!(matches!(
                &driver.source,
                DriverSource::Primitive { name } if name == &call.name
            ));
            assert_eq!(emitted.target, driver.target, "{path}");
            assert!(
                !lowered
                    .cell
                    .registers
                    .iter()
                    .any(|register| register.name == emitted.target),
                "{path}"
            );

            let source = call.args[1].as_ref().unwrap();
            let gate = call.args[2].as_ref().unwrap();
            assert_eq!(
                expand_expr(&operands[0], &temp_exprs),
                expected_expr(source),
                "source topology changed in {path}"
            );
            assert_eq!(
                expand_expr(&operands[1], &temp_exprs),
                expected_expr(gate),
                "gate topology changed in {path}"
            );

            let delay0 = if let Some(delay) = &call.delay {
                assert_eq!(delay.values.len(), 3, "{path}");
                assert_eq!(emitted.delay.len(), delay.values.len(), "{path}");
                let aliases = delay
                    .values
                    .iter()
                    .map(|component| {
                        scalar_symbol(component.as_ref().unwrap())
                            .expect("corpus transistor delay component alias")
                    })
                    .collect::<Vec<_>>();
                for (component, alias) in emitted.delay.components().zip(&aliases) {
                    assert_eq!(
                        component, &lowered.timing_aliases[alias],
                        "delay component `{alias}` changed in {path}"
                    );
                }
                aliases[0].clone()
            } else {
                assert!(is_zero_delay(&emitted.delay), "{path}");
                "0".to_string()
            };

            *totals
                .transistor_counts
                .entry(call.name.clone())
                .or_default() += 1;
            totals.rows.push(format!(
                "{} | {} | source-order={} | span={}:{} | drain={} | source={} | gate={} | delay0={} | strength=none | assignment-index={} | emitted={} | emitted-delay={}",
                path,
                call.name,
                analyzed.source_order,
                call.span.line,
                call.span.column,
                analyzed.args[0].as_deref().unwrap(),
                analyzed.args[1].as_deref().unwrap(),
                analyzed.args[2].as_deref().unwrap(),
                delay0,
                assignment_index,
                render_expr(&emitted.expr),
                render_delay_tuple(&emitted.delay)
            ));
        }
    }
    totals
}

fn is_zero_delay(delay: &DelayTuple) -> bool {
    delay.len() == 1 && delay.first().as_expr() == &Expr::atom("0")
}

fn assert_exact_mode(totals: &ModeTotals, mode: GenerateMode) {
    assert_eq!(totals.succeeded, 206);
    assert_eq!(
        totals
            .transistor_files
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        TRANSISTOR_FILES
    );
    assert_eq!(
        totals.transistor_counts,
        BTreeMap::from([
            ("nmos".to_string(), 17),
            ("pmos".to_string(), 7),
            ("rnmos".to_string(), 1),
        ])
    );
    assert_eq!(totals.warnings, 0);
    assert_eq!(
        totals.ignores,
        match mode {
            GenerateMode::Delayful | GenerateMode::Nodelay => 49,
        }
    );
    assert_eq!(
        totals.assignments,
        match mode {
            GenerateMode::Delayful => 1958,
            GenerateMode::Nodelay => 1955,
        }
    );
    assert_eq!(totals.temporaries, 1168);
    assert_eq!(totals.register_cells, 27);
    assert_eq!(totals.registers, 48);
    assert_eq!(totals.rows.len(), 25);
}

fn is_transistor_name(name: &str) -> bool {
    matches!(name, "nmos" | "pmos" | "rnmos")
}

fn operator_for_name(name: &str) -> ValueOperator {
    match name {
        "nmos" => ValueOperator::Nmos,
        "pmos" => ValueOperator::Pmos,
        "rnmos" => ValueOperator::Rnmos,
        _ => panic!("not a transistor primitive: {name}"),
    }
}

fn transistor_operation(expr: &Expr) -> Option<(ValueOperator, &[Expr])> {
    let Expr::List(items) = expr else {
        return None;
    };
    let (Expr::Atom(head), operands) = items.split_first()? else {
        return None;
    };
    let operator = ValueOperator::parse(head)?;
    matches!(
        operator,
        ValueOperator::Nmos | ValueOperator::Pmos | ValueOperator::Rnmos
    )
    .then_some((operator, operands))
}

fn expand_expr(expr: &Expr, temporaries: &BTreeMap<&str, &Expr>) -> String {
    match expr {
        Expr::Atom(atom) => temporaries
            .get(atom.as_str())
            .map(|expr| expand_expr(expr, temporaries))
            .unwrap_or_else(|| atom.clone()),
        Expr::List(items) => format!(
            "({})",
            items
                .iter()
                .map(|item| expand_expr(item, temporaries))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

fn expected_expr(expr: &SvExpr) -> String {
    match &expr.kind {
        ExprKind::Path(parts) => parts.join("::"),
        ExprKind::Integer(value) | ExprKind::Real(value) => value.clone(),
        ExprKind::Constant(kind) => match kind {
            ConstKind::Zero => "0".to_string(),
            ConstKind::One => "1".to_string(),
            ConstKind::X => "x".to_string(),
            ConstKind::Z => "z".to_string(),
        },
        ExprKind::Group(inner) => expected_expr(inner),
        ExprKind::Unary { op, expr } => match op {
            UnaryOp::Not | UnaryOp::BitNot => format!("(not {})", expected_expr(expr)),
            UnaryOp::Plus => expected_expr(expr),
            UnaryOp::Minus => format!("(neg {})", expected_expr(expr)),
        },
        ExprKind::Binary {
            op: BinaryOp::BitAnd | BinaryOp::LogicalAnd,
            left: _,
            right: _,
        } => {
            let mut operands = Vec::new();
            collect_associative(expr, true, &mut operands);
            format!("(and {})", operands.join(" "))
        }
        ExprKind::Binary {
            op: BinaryOp::BitOr | BinaryOp::LogicalOr,
            left: _,
            right: _,
        } => {
            let mut operands = Vec::new();
            collect_associative(expr, false, &mut operands);
            format!("(or {})", operands.join(" "))
        }
        ExprKind::Binary { op, left, right } => {
            let operator = match op {
                BinaryOp::CaseEq => "caseeq",
                BinaryOp::CaseNeq => "caseneq",
                BinaryOp::Eq => "eq",
                BinaryOp::Neq => "neq",
                BinaryOp::BitXor => "xor",
                BinaryOp::BitNand => "nand",
                BinaryOp::BitNor => "nor",
                BinaryOp::BitXnor => "xnor",
                other => panic!("unexpected corpus transistor value operator {other:?}"),
            };
            format!(
                "({operator} {} {})",
                expected_expr(left),
                expected_expr(right)
            )
        }
        other => panic!("unexpected corpus transistor value expression {other:?}"),
    }
}

fn collect_associative(expr: &SvExpr, and: bool, output: &mut Vec<String>) {
    let matches_operator = |op: BinaryOp| {
        if and {
            matches!(op, BinaryOp::BitAnd | BinaryOp::LogicalAnd)
        } else {
            matches!(op, BinaryOp::BitOr | BinaryOp::LogicalOr)
        }
    };
    match &expr.kind {
        ExprKind::Binary { op, left, right } if matches_operator(*op) => {
            collect_associative(left, and, output);
            collect_associative(right, and, output);
        }
        _ => output.push(expected_expr(expr)),
    }
}

fn scalar_symbol(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(parts) if parts.len() == 1 => Some(parts[0].clone()),
        ExprKind::Group(inner) => scalar_symbol(inner),
        _ => None,
    }
}

fn assert_or_update_fixture(actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transistor/corpus_summary.transistor");
    if std::env::var_os("UPDATE_TRANSISTOR_CORPUS_GOLDEN").is_some() {
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(actual, expected, "transistor corpus summary changed");
}
