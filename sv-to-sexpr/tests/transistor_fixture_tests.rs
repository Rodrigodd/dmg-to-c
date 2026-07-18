use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sv_to_sexpr::analyze::{AnalysisReport, DriverSource};
use sv_to_sexpr::diagnostic::{Diagnostic, DiagnosticKind};
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{Assignment, CellItem, Expr, LoweredModule, ValueOperator};
use sv_to_sexpr::serialize::{render_cell, render_expr};
use sv_to_sexpr::survey::{
    analyze_file_with_sibling_catalog_and_generate_mode,
    lower_file_with_sibling_catalog_and_generate_mode,
};

#[derive(Clone, Copy)]
struct ExpectedTransistor {
    source_order: usize,
    target: &'static str,
    operator: ValueOperator,
    source: &'static str,
    source_gate_syntax: &'static str,
    gate_atom: &'static str,
    delay: &'static str,
    dependencies: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    path: &'static str,
    registers: &'static [&'static str],
    warnings: usize,
    intentional_ignores: usize,
    transistors: &'static [ExpectedTransistor],
}

const DLATCH_TRANSISTORS: &[ExpectedTransistor] = &[ExpectedTransistor {
    source_order: 4,
    target: "gated_q",
    operator: ValueOperator::Rnmos,
    source: "ena_q_n",
    source_gate_syntax: "(and q_n ((caseneq ena_q_n 1)))",
    gate_atom: "t7",
    delay: "0",
    dependencies: &[("t6", "(caseneq ena_q_n 1)"), ("t7", "(and q_n t6)")],
}];

const IDU_BIT0_TRANSISTORS: &[ExpectedTransistor] = &[ExpectedTransistor {
    source_order: 5,
    target: "aoi_buf_y",
    operator: ValueOperator::Nmos,
    source: "aoi_y",
    source_gate_syntax: "(and aoi_buf_ena ((caseneq aoi_y 1)))",
    gate_atom: "t3",
    delay: "(elmore (wire L_aoi_buf_y) (nmos 17))",
    dependencies: &[("t2", "(caseneq aoi_y 1)"), ("t3", "(and aoi_buf_ena t2)")],
}];

const IDU_BIT123456_TRANSISTORS: &[ExpectedTransistor] = &[
    ExpectedTransistor {
        source_order: 1,
        target: "chain_a_y",
        operator: ValueOperator::Nmos,
        source: "chain_a_in",
        source_gate_syntax: "(and chain_a_ena ((caseneq chain_a_in 1)))",
        gate_atom: "t1",
        delay: "(elmore (wire L_chain_a_y) (nmos 17))",
        dependencies: &[
            ("t0", "(caseneq chain_a_in 1)"),
            ("t1", "(and chain_a_ena t0)"),
        ],
    },
    ExpectedTransistor {
        source_order: 4,
        target: "chain_b_y",
        operator: ValueOperator::Nmos,
        source: "chain_b_in",
        source_gate_syntax: "(and chain_b_ena ((caseneq chain_b_in 1)))",
        gate_atom: "t3",
        delay: "(elmore (wire L_chain_b_y) (nmos 17))",
        dependencies: &[
            ("t2", "(caseneq chain_b_in 1)"),
            ("t3", "(and chain_b_ena t2)"),
        ],
    },
];

const IRQ_PRIO_BIT0_TRANSISTORS: &[ExpectedTransistor] = &[
    ExpectedTransistor {
        source_order: 5,
        target: "dist_nand_a_y_n",
        operator: ValueOperator::Nmos,
        source: "dist_nand_a_in_n",
        source_gate_syntax: "(and (and dist_nand_a_in1 dist_nand_a_in2) ((caseneq dist_nand_a_in_n 1)))",
        gate_atom: "t4",
        delay: "(elmore (wire L_dist_nand_a_y_n) (* (nmos 3) 2))",
        dependencies: &[
            ("t3", "(caseneq dist_nand_a_in_n 1)"),
            ("t4", "(and dist_nand_a_in1 dist_nand_a_in2 t3)"),
        ],
    },
    ExpectedTransistor {
        source_order: 7,
        target: "dist_nand_b_y_n",
        operator: ValueOperator::Nmos,
        source: "dist_nand_b_in_n",
        source_gate_syntax: "(and dist_nand_b_in ((caseneq dist_nand_b_in_n 1)))",
        gate_atom: "t6",
        delay: "(elmore (wire L_dist_nand_b_y_n) (nmos 3))",
        dependencies: &[
            ("t5", "(caseneq dist_nand_b_in_n 1)"),
            ("t6", "(and dist_nand_b_in t5)"),
        ],
    },
    ExpectedTransistor {
        source_order: 8,
        target: "dist_nor_y_p",
        operator: ValueOperator::Pmos,
        source: "dist_nor_in_p",
        source_gate_syntax: "(or dist_nor_in ((caseeq dist_nor_in_p 0)))",
        gate_atom: "t8",
        delay: "(elmore (wire L_dist_nor_y_p) (pmos 5))",
        dependencies: &[
            ("t7", "(caseeq dist_nor_in_p 0)"),
            ("t8", "(or dist_nor_in t7)"),
        ],
    },
];

const CASES: &[Case] = &[
    Case {
        name: "dlatch_ee_irq",
        path: "sv-cells/sm83/cells/dlatch_ee_irq.sv",
        registers: &["q_n"],
        warnings: 0,
        intentional_ignores: 5,
        transistors: DLATCH_TRANSISTORS,
    },
    Case {
        name: "idu_bit0",
        path: "sv-cells/sm83/cells/idu_bit0.sv",
        registers: &[],
        warnings: 0,
        intentional_ignores: 21,
        transistors: IDU_BIT0_TRANSISTORS,
    },
    Case {
        name: "idu_bit123456",
        path: "sv-cells/sm83/cells/idu_bit123456.sv",
        registers: &[],
        warnings: 0,
        intentional_ignores: 18,
        transistors: IDU_BIT123456_TRANSISTORS,
    },
    Case {
        name: "irq_prio_bit0",
        path: "sv-cells/sm83/cells/irq_prio_bit0.sv",
        registers: &[],
        warnings: 0,
        intentional_ignores: 19,
        transistors: IRQ_PRIO_BIT0_TRANSISTORS,
    },
];

#[test]
fn reviewed_transistor_goldens_preserve_kind_topology_order_timing_and_diagnostics() {
    for case in CASES {
        let delayful = configured(case.path, GenerateMode::Delayful);
        let nodelay = configured(case.path, GenerateMode::Nodelay);
        assert_eq!(
            delayful, nodelay,
            "{} changed with generate mode",
            case.path
        );
        let (analysis, lowered) = delayful;
        lowered.cell.validate().unwrap();
        assert_transistor_case(case, &analysis, &lowered);

        assert_or_update_fixture(
            case.name,
            "analysis",
            &normalize_repository_paths(analysis.render()),
        );
        assert_or_update_fixture(case.name, "ir", &render_cli_ir(&lowered));
        assert_or_update_fixture(case.name, "cell", &render_cell(&lowered.cell));
        assert_or_update_fixture(
            case.name,
            "diagnostics",
            &render_diagnostics(&lowered.diagnostics),
        );
    }
}

#[test]
fn cli_lower_and_convert_match_transistor_goldens_in_both_modes() {
    for case in CASES {
        for nodelay in [false, true] {
            let mut lower_args = vec!["lower", case.path];
            if nodelay {
                lower_args.push("--nodelay");
            }
            let lower = run_cli(&lower_args);
            assert!(
                lower.status.success(),
                "lower failed for {}: {}",
                case.path,
                String::from_utf8_lossy(&lower.stderr)
            );
            assert_eq!(
                String::from_utf8(lower.stdout).unwrap(),
                fixture(case.name, "ir")
            );
            assert_cli_diagnostics(case, &lower.stderr);

            let output = temporary_output(case.name, nodelay);
            let mut convert_args = vec![
                "convert-file",
                "--dry-run",
                case.path,
                output.to_str().unwrap(),
            ];
            if nodelay {
                convert_args.push("--nodelay");
            }
            let convert = run_cli(&convert_args);
            assert!(
                convert.status.success(),
                "convert failed for {}: {}",
                case.path,
                String::from_utf8_lossy(&convert.stderr)
            );
            assert_eq!(
                String::from_utf8(convert.stdout).unwrap(),
                fixture(case.name, "cell")
            );
            assert_cli_diagnostics(case, &convert.stderr);
            assert!(!output.exists());
        }

        let strict = run_cli(&["lower", case.path, "--strict"]);
        assert!(strict.status.success());
        let strict_stderr = String::from_utf8_lossy(&strict.stderr);
        assert_eq!(strict_stderr.matches(": warning:").count(), case.warnings);
        assert_eq!(
            strict_stderr.matches(": intentional-ignore:").count(),
            case.intentional_ignores
        );
        assert!(!strict_stderr.to_ascii_lowercase().contains("transistor"));
    }
}

#[test]
fn transistor_cell_goldens_are_canonical_for_sibling_formatter() {
    for case in CASES {
        let cell = fixture_path(case.name, "cell");
        let result = Command::new("cargo")
            .current_dir(repository_root())
            .args([
                "run",
                "--quiet",
                "--manifest-path",
                "sexpr-fmt/Cargo.toml",
                "--",
                "--check",
                cell.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "formatter found non-canonical {}: {}",
            cell.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
    }
}

fn configured(path: &str, mode: GenerateMode) -> (AnalysisReport, LoweredModule) {
    let physical = repository_root().join(path);
    let analysis = analyze_file_with_sibling_catalog_and_generate_mode(&physical, mode).unwrap();
    let lowered = lower_file_with_sibling_catalog_and_generate_mode(&physical, mode).unwrap();
    (analysis, lowered)
}

fn assert_transistor_case(case: &Case, analysis: &AnalysisReport, lowered: &LoweredModule) {
    let module = &analysis.modules[0];
    assert_eq!(
        module
            .registers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        case.registers
    );
    assert!(analysis.requirements.iter().all(|requirement| {
        requirement.capability_id != "primitive.transistor"
            && requirement.milestone.label() != "M11"
    }));

    let calls = module
        .primitive_calls
        .iter()
        .filter(|call| matches!(call.name.as_str(), "nmos" | "pmos" | "rnmos"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), case.transistors.len());
    let drivers = module
        .drivers
        .iter()
        .filter(|driver| {
            matches!(
                &driver.source,
                DriverSource::Primitive { name }
                    if matches!(name.as_str(), "nmos" | "pmos" | "rnmos")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(drivers.len(), case.transistors.len());

    for ((call, driver), expected) in calls.iter().zip(drivers).zip(case.transistors) {
        assert_eq!(call.source_order, expected.source_order);
        assert_eq!(call.name, expected.operator.as_str());
        assert!(call.strength.is_none());
        assert_eq!(call.args[0].as_deref(), Some(expected.target));
        assert_eq!(call.args[1].as_deref(), Some(expected.source));
        assert_eq!(call.args[2].as_deref(), Some(expected.source_gate_syntax));
        assert_eq!(driver.source_order, expected.source_order);
        assert_eq!(driver.target, expected.target);
        assert!(matches!(
            &driver.source,
            DriverSource::Primitive { name } if name == expected.operator.as_str()
        ));
    }

    let assignments = assignments(lowered);
    let transistor_assignments = assignments
        .iter()
        .enumerate()
        .filter_map(|(index, assignment)| {
            transistor_operation(&assignment.expr).map(|operation| (index, *assignment, operation))
        })
        .collect::<Vec<_>>();
    assert_eq!(transistor_assignments.len(), case.transistors.len());
    for ((index, assignment, (operator, operands)), expected) in
        transistor_assignments.into_iter().zip(case.transistors)
    {
        assert_eq!(assignment.target, expected.target);
        assert_eq!(operator, expected.operator);
        assert_eq!(
            operands,
            [Expr::atom(expected.source), Expr::atom(expected.gate_atom)]
        );
        assert_eq!(render_expr(&assignment.delay), expected.delay);
        assert!(index >= expected.dependencies.len());
        for (actual, (target, expression)) in assignments
            [index - expected.dependencies.len()..index]
            .iter()
            .zip(expected.dependencies)
        {
            assert_eq!(actual.target, *target);
            assert_eq!(render_expr(&actual.expr), *expression);
            assert_eq!(actual.delay, Expr::atom("0"));
        }
    }

    for assignment in &assignments {
        if let Expr::List(items) = &assignment.expr {
            assert!(
                items
                    .iter()
                    .skip(1)
                    .all(|item| matches!(item, Expr::Atom(_)))
            );
        }
    }
    assert_eq!(
        diagnostic_count(&lowered.diagnostics, DiagnosticKind::Warning),
        case.warnings
    );
    assert_eq!(
        diagnostic_count(&lowered.diagnostics, DiagnosticKind::IntentionalIgnore),
        case.intentional_ignores
    );
    assert!(lowered.diagnostics.iter().all(|diagnostic| {
        let message = diagnostic.message.to_ascii_lowercase();
        !message.contains("transistor")
            && !message.contains("unsupported primitive nmos")
            && !message.contains("unsupported primitive pmos")
            && !message.contains("unsupported primitive rnmos")
    }));
}

fn assignments(lowered: &LoweredModule) -> Vec<&Assignment> {
    lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect()
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

fn diagnostic_count(diagnostics: &[Diagnostic], kind: DiagnosticKind) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == kind)
        .count()
}

fn assert_cli_diagnostics(case: &Case, bytes: &[u8]) {
    let stderr = String::from_utf8_lossy(bytes);
    assert_eq!(stderr.matches(": warning:").count(), case.warnings);
    assert_eq!(
        stderr.matches(": intentional-ignore:").count(),
        case.intentional_ignores
    );
    assert!(!stderr.to_ascii_lowercase().contains("transistor"));
    assert!(!stderr.contains("unsupported primitive nmos"));
    assert!(!stderr.contains("unsupported primitive pmos"));
    assert!(!stderr.contains("unsupported primitive rnmos"));
}

fn render_cli_ir(lowered: &LoweredModule) -> String {
    format!(
        "cell:\n{:#?}\ntiming aliases:\n{:#?}\n",
        lowered.cell, lowered.timing_aliases
    )
}

fn render_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "diagnostics: []\n".to_string();
    }
    let mut output = String::from("diagnostics:\n");
    for diagnostic in diagnostics {
        writeln!(
            &mut output,
            "  {} | {}:{}:{} | {}",
            diagnostic.kind,
            logical_path(&diagnostic.span.path),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.message
        )
        .unwrap();
    }
    output
}

fn logical_path(path: &Path) -> String {
    path.strip_prefix(repository_root())
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_repository_paths(rendered: String) -> String {
    rendered.replace(&format!("{}/", repository_root().display()), "")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture_path(name: &str, extension: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transistor")
        .join(format!("{name}.{extension}"))
}

fn fixture(name: &str, extension: &str) -> String {
    fs::read_to_string(fixture_path(name, extension)).unwrap()
}

fn assert_or_update_fixture(name: &str, extension: &str, actual: &str) {
    let path = fixture_path(name, extension);
    if std::env::var_os("UPDATE_TRANSISTOR_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "transistor fixture {} changed",
        path.display()
    );
}

fn temporary_output(name: &str, nodelay: bool) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sv-to-sexpr-transistor-{name}-{nodelay}-{}-{:?}.cell",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    path
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .current_dir(repository_root())
        .args(args)
        .output()
        .unwrap()
}
