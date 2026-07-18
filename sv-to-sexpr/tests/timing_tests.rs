use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticPolicy, Span};
use sv_to_sexpr::ir::{Assignment, CellItem, LoweredModule};
use sv_to_sexpr::lower::lower_file;
use sv_to_sexpr::serialize::{render_cell, render_expr};

struct TimingCase {
    name: &'static str,
    source: &'static str,
    expected_assignments: &'static [(&'static str, &'static str, &'static str)],
    warnings: usize,
    intentional_ignores: usize,
}

const CASES: &[TimingCase] = &[
    TimingCase {
        name: "single_path",
        source: "sv-to-sexpr/tests/fixtures/timing/single_path.sv",
        expected_assignments: &[("y", "(and a b)", "T_single")],
        warnings: 0,
        intentional_ignores: 2,
    },
    TimingCase {
        name: "ambiguous_paths",
        source: "sv-to-sexpr/tests/fixtures/timing/ambiguous_paths.sv",
        expected_assignments: &[("y", "(or a b)", "T_first")],
        warnings: 0,
        intentional_ignores: 3,
    },
    TimingCase {
        name: "explicit_precedence",
        source: "sv-to-sexpr/tests/fixtures/timing/explicit_precedence.sv",
        expected_assignments: &[("y", "a", "T_explicit")],
        warnings: 0,
        intentional_ignores: 2,
    },
    TimingCase {
        name: "procedural_state",
        source: "sv-to-sexpr/tests/fixtures/timing/procedural_state.sv",
        expected_assignments: &[("t0", "(and a b)", "0"), ("q", "(mux ena t0 q)", "T_state")],
        warnings: 0,
        intentional_ignores: 2,
    },
];

#[test]
fn reviewed_timing_goldens_cover_specify_selection_precedence_state_and_diagnostics() {
    for case in CASES {
        let first = lower_repository_file(case.source).unwrap();
        let second = lower_repository_file(case.source).unwrap();
        assert_eq!(
            first, second,
            "nondeterministic lowering for {}",
            case.source
        );
        first.cell.validate().unwrap();
        assert_eq!(
            assignment_triplets(&first),
            case.expected_assignments
                .iter()
                .map(|(target, value, delay)| {
                    (*target, (*value).to_string(), (*delay).to_string())
                })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
                .count(),
            case.warnings
        );
        assert_eq!(
            first
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
                .count(),
            case.intentional_ignores
        );
        assert_or_update_fixture(case.name, "cell", &render_cell(&first.cell));
        assert_or_update_fixture(
            case.name,
            "diagnostics",
            &render_diagnostics(&first.diagnostics),
        );
    }
}

#[test]
fn reference_cell_has_exact_first_applicable_q_q_n_and_d_assignments() {
    let source = "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv";
    let first = lower_repository_file(source).unwrap();
    let second = lower_repository_file(source).unwrap();
    assert_eq!(first, second);
    first.cell.validate().unwrap();

    let assignments = assignments(&first);
    let exact = |target: &str| {
        let assignment = assignments
            .iter()
            .find(|assignment| assignment.target == target)
            .unwrap_or_else(|| panic!("missing reference assignment {target}"));
        (
            render_expr(&assignment.expr),
            render_expr(&assignment.delay),
        )
    };
    assert_eq!(
        exact("q_n"),
        (
            "(mux t19 ff2 q_n)".to_string(),
            "(+ (+ (elmore (wire 55) (* (pmos 3) 2)) (elmore (wire 25) (* (nmos 3) 2))) (elmore (wire L_q_n) (pmos 13)))".to_string(),
        )
    );
    assert_eq!(
        exact("q"),
        (
            "(not q_n)".to_string(),
            "(+ (+ (+ (elmore (wire 55) (* (nmos 3) 2)) (elmore (wire 25) (* (pmos 3) 2))) (elmore (wire L_q_n) (nmos 6))) (elmore (wire L_q) (pmos 13)))".to_string(),
        )
    );
    assert_eq!(
        exact("d"),
        (
            "(bufif0-strength 1 pch_n strong1 highz0)".to_string(),
            "(elmore (wire L_d) (pmos 5))".to_string(),
        )
    );
    assert_eq!(
        first
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
            .count(),
        0
    );
    assert_eq!(
        first
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
            .count(),
        7
    );
    assert_or_update_fixture("reference", "cell", &render_cell(&first.cell));
    assert_or_update_fixture(
        "reference",
        "diagnostics",
        &render_diagnostics(&first.diagnostics),
    );
}

#[test]
fn specify_paths_reject_non_scalar_controls_and_targets_at_exact_spans() {
    let control = lower_file(
        Path::new("bad_control.sv"),
        "module bad(input logic a, b, output logic y); assign y = a; specify (a & b *> y) = (1); endspecify endmodule",
    )
    .unwrap_err();
    assert_eq!(control.span, Span::new("bad_control.sv", 1, 70));
    assert_eq!(
        control.message,
        "specify path control must be a scalar symbol"
    );

    let target = lower_file(
        Path::new("bad_target.sv"),
        "module bad(input logic a, b, output logic y); assign y = a; specify (a *> y & b) = (1); endspecify endmodule",
    )
    .unwrap_err();
    assert_eq!(target.span, Span::new("bad_target.sv", 1, 75));
    assert_eq!(
        target.message,
        "specify path target must be a scalar symbol"
    );
}

#[test]
fn specify_tuples_require_entry_zero_and_ignore_additional_path_once_per_repeated_target() {
    for (source, expected_message) in [
        (
            "module bad(input logic a, output logic y); assign y = a; specify (a *> y) = (); endspecify endmodule",
            "delay tuple must contain a first entry",
        ),
        (
            "module bad(input logic a, output logic y); assign y = a; specify (a *> y) = (, 2); endspecify endmodule",
            "explicitly omitted first delay tuple entry is unsupported",
        ),
    ] {
        let error = lower_file(Path::new("bad_tuple.sv"), source).unwrap_err();
        assert_eq!(error.span, Span::new("bad_tuple.sv", 1, 66));
        assert_eq!(error.message, expected_message);
    }

    let repeated = lower_file(
        Path::new("repeated.sv"),
        "module repeated(input logic a, b, output logic y); assign y = a; assign y = b; specify (a *> y) = (T_first); (b *> y) = (T_second); endspecify endmodule",
    )
    .unwrap();
    assert_eq!(
        assignment_triplets(&repeated),
        vec![
            ("y", "a".to_string(), "T_first".to_string()),
            ("y", "b".to_string(), "T_first".to_string()),
        ]
    );
    let additional_path_ignores = repeated
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.kind == DiagnosticKind::IntentionalIgnore
                && diagnostic
                    .message
                    .starts_with("additional control-dependent specify path")
        })
        .collect::<Vec<_>>();
    assert_eq!(additional_path_ignores.len(), 1);
    assert_eq!(
        additional_path_ignores[0].span,
        Span::new("repeated.sv", 1, 110)
    );
    assert_eq!(
        additional_path_ignores[0].message,
        "additional control-dependent specify path for target `y` is intentionally ignored because the one-delay cell DSL selects the first source-ordered path for the target"
    );
    assert!(!DiagnosticPolicy::new(false).is_failure(additional_path_ignores[0]));
    assert!(!DiagnosticPolicy::new(true).is_failure(additional_path_ignores[0]));
    assert_eq!(
        repeated
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
            .count(),
        0
    );
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn lower_repository_file(logical_path: &str) -> Result<LoweredModule, Diagnostic> {
    let input = fs::read_to_string(repository_root().join(logical_path)).unwrap();
    lower_file(Path::new(logical_path), &input)
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

fn assignment_triplets(lowered: &LoweredModule) -> Vec<(&str, String, String)> {
    assignments(lowered)
        .into_iter()
        .map(|assignment| {
            (
                assignment.target.as_str(),
                render_expr(&assignment.expr),
                render_expr(&assignment.delay),
            )
        })
        .collect()
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
            diagnostic.span.path.display(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.message
        )
        .unwrap();
    }
    output
}

fn assert_or_update_fixture(name: &str, extension: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing")
        .join(format!("{name}.{extension}"));
    if std::env::var_os("UPDATE_TIMING_GOLDENS").is_some() {
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual,
        expected,
        "timing fixture {} changed",
        fixture.display()
    );
}
