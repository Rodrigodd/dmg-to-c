use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sv_to_sexpr::analyze::{
    ConnectionSource, InstantiationResolution, ParameterBindingSource, ResolvedInstantiation,
};
use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, Expr, LoweredModule};
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::{
    analyze_file_with_sibling_catalog_and_generate_mode,
    lower_file_with_sibling_catalog_and_generate_mode,
};

const HALF_ADD: &str = "sv-cells/dmg_cpu_b/cells/half_add.sv";
const FULL_ADD: &str = "sv-cells/dmg_cpu_b/cells/full_add.sv";

#[test]
fn actual_adders_have_exact_resolved_hierarchy_and_flattened_goldens() {
    for case in cases() {
        let first = configured(case.path, GenerateMode::Delayful);
        let second = configured(case.path, GenerateMode::Delayful);
        assert_eq!(first.analysis, second.analysis);
        assert_eq!(first.lowered, second.lowered);
        first.lowered.cell.validate().unwrap();
        assert_flat_values(&first.lowered);
        assert_case(case, &first);

        assert_fixture(case.name, "analysis", &render_analysis(&first.analysis));
        assert_fixture(case.name, "ir", &render_cli_ir(&first.lowered));
        assert_fixture(case.name, "cell", &render_cell(&first.lowered.cell));
        assert_fixture(
            case.name,
            "diagnostics",
            &render_diagnostics(&first.lowered.diagnostics),
        );
    }
}

#[test]
fn adders_are_generate_mode_invariant() {
    for case in cases() {
        let delayful = configured(case.path, GenerateMode::Delayful);
        let nodelay = configured(case.path, GenerateMode::Nodelay);
        assert_eq!(delayful.analysis, nodelay.analysis);
        assert_eq!(delayful.lowered, nodelay.lowered);
    }
}

#[test]
fn cli_lower_and_convert_use_sibling_catalog_and_match_goldens() {
    for case in cases() {
        let expected_ir = fixture(case.name, "ir");
        let expected_cell = fixture(case.name, "cell");
        let expected_analysis = fixture(case.name, "analysis");
        for nodelay in [false, true] {
            let mut analyze_args = vec!["analyze", case.path];
            if nodelay {
                analyze_args.push("--nodelay");
            }
            let analyze = run_cli(&analyze_args);
            assert!(
                analyze.status.success(),
                "analyze failed for {}: {}",
                case.name,
                String::from_utf8_lossy(&analyze.stderr)
            );
            assert_eq!(
                String::from_utf8(analyze.stdout).unwrap(),
                expected_analysis
            );

            let mut lower_args = vec!["lower", case.path];
            if nodelay {
                lower_args.push("--nodelay");
            }
            let lower = run_cli(&lower_args);
            assert!(
                lower.status.success(),
                "lower failed for {}: {}",
                case.name,
                String::from_utf8_lossy(&lower.stderr)
            );
            assert_eq!(String::from_utf8(lower.stdout).unwrap(), expected_ir);
            assert_eq!(
                String::from_utf8(lower.stderr)
                    .unwrap()
                    .matches(": intentional-ignore:")
                    .count(),
                0
            );

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
                case.name,
                String::from_utf8_lossy(&convert.stderr)
            );
            assert_eq!(String::from_utf8(convert.stdout).unwrap(), expected_cell);
            assert!(!output.exists());
        }
    }
}

#[test]
fn hierarchy_cell_goldens_are_canonical_for_sibling_formatter() {
    let root = repository_root();
    for case in cases() {
        let cell = fixture_path(case.name, "cell");
        let result = Command::new("cargo")
            .current_dir(&root)
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
            "sexpr-fmt found non-canonical {}: {}",
            case.name,
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
    }
}

struct ConfiguredResult {
    analysis: sv_to_sexpr::analyze::AnalysisReport,
    lowered: LoweredModule,
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    path: &'static str,
    instance_count: usize,
    targets: &'static [&'static str],
}

fn cases() -> [Case; 2] {
    [
        Case {
            name: "half_add",
            path: HALF_ADD,
            instance_count: 2,
            targets: &["cout", "sum"],
        },
        Case {
            name: "full_add",
            path: FULL_ADD,
            instance_count: 5,
            targets: &["sum", "caxb", "cout", "ab", "axb"],
        },
    ]
}

fn configured(path: &str, mode: GenerateMode) -> ConfiguredResult {
    let physical_path = repository_root().join(path);
    let analysis =
        analyze_file_with_sibling_catalog_and_generate_mode(&physical_path, mode).unwrap();
    let lowered = lower_file_with_sibling_catalog_and_generate_mode(&physical_path, mode).unwrap();
    ConfiguredResult { analysis, lowered }
}

fn assert_case(case: Case, result: &ConfiguredResult) {
    let module = &result.analysis.modules[0];
    assert_eq!(module.instantiations.len(), case.instance_count);
    assert_eq!(source_targets(&result.lowered), case.targets);
    assert!(result.lowered.cell.registers.is_empty());
    assert!(result.lowered.diagnostics.is_empty());

    match case.name {
        "half_add" => {
            assert_instance(
                resolved(module, 0),
                "dmg_and2",
                "and2_cout_inst",
                "L_cout",
                &[("y", "cout"), ("in1", "a"), ("in2", "b")],
            );
            assert_instance(
                resolved(module, 1),
                "dmg_xor",
                "xor_sum_inst",
                "L_sum",
                &[("y", "sum"), ("in1", "a"), ("in2", "b")],
            );
            assert!(format!("{:#?}", result.lowered).contains("L_cout"));
            assert!(format!("{:#?}", result.lowered).contains("L_sum"));
        }
        "full_add" => {
            let expected = [
                (
                    "dmg_xor",
                    "xor_sum_inst",
                    "L_sum",
                    [("y", "sum"), ("in1", "axb"), ("in2", "cin")],
                ),
                (
                    "dmg_nand2",
                    "nand2_caxb_inst",
                    "120",
                    [("y", "caxb"), ("in1", "cin"), ("in2", "axb")],
                ),
                (
                    "dmg_nand2",
                    "nand2_cout_inst",
                    "L_cout",
                    [("y", "cout"), ("in1", "ab"), ("in2", "caxb")],
                ),
                (
                    "dmg_nand2",
                    "nand2_ab_inst",
                    "119",
                    [("y", "ab"), ("in1", "b"), ("in2", "a")],
                ),
                (
                    "dmg_xor",
                    "xor_axb_inst",
                    "296",
                    [("y", "axb"), ("in1", "a"), ("in2", "b")],
                ),
            ];
            for (index, (module_name, instance, delay, connections)) in
                expected.into_iter().enumerate()
            {
                assert_instance(
                    resolved(module, index),
                    module_name,
                    instance,
                    delay,
                    &connections,
                );
            }
            let debug = format!("{:#?}", result.lowered);
            for expected in ["L_sum", "120", "L_cout", "119", "296"] {
                assert!(debug.contains(expected), "missing delay binding {expected}");
            }
        }
        _ => unreachable!(),
    }

    let debug = format!("{:#?}", result.lowered);
    for leaked in ["L_y", "\"in1\"", "\"in2\"", "\"y\""] {
        assert!(!debug.contains(leaked), "flattened IR leaked {leaked}");
    }
    for alias in result.lowered.timing_aliases.keys() {
        assert!(alias.contains("__"), "unqualified child alias {alias}");
        assert!(
            module
                .instantiations
                .iter()
                .any(|instance| alias.starts_with(&format!("{}__", instance.instance)))
        );
    }
}

fn resolved(
    module: &sv_to_sexpr::analyze::ModuleAnalysis,
    index: usize,
) -> (
    &sv_to_sexpr::analyze::InstantiationAnalysis,
    &ResolvedInstantiation,
) {
    let instance = &module.instantiations[index];
    let InstantiationResolution::Resolved(resolution) = &instance.resolution else {
        panic!("{} did not resolve", instance.instance)
    };
    (instance, resolution)
}

fn assert_instance(
    (instance, resolution): (
        &sv_to_sexpr::analyze::InstantiationAnalysis,
        &ResolvedInstantiation,
    ),
    module: &str,
    name: &str,
    parameter: &str,
    expected_connections: &[(&str, &str)],
) {
    assert_eq!(instance.module, module);
    assert_eq!(instance.instance, name);
    assert_eq!(resolution.parameter_bindings.len(), 1);
    let binding = &resolution.parameter_bindings[0];
    assert_eq!(binding.parameter, "L_y");
    assert_eq!(binding.source, ParameterBindingSource::Named);
    assert_eq!(expression_atom(&binding.expression), parameter);
    assert_eq!(resolution.connections.len(), expected_connections.len());
    for (connection, (port, expression)) in resolution
        .connections
        .iter()
        .zip(expected_connections.iter())
    {
        assert_eq!(connection.port, *port);
        assert_eq!(connection.source, ConnectionSource::Named);
        assert_eq!(expression_atom(&connection.expression), *expression);
        assert_eq!(
            connection.direction,
            if *port == "y" {
                sv_to_sexpr::ast::Direction::Output
            } else {
                sv_to_sexpr::ast::Direction::Input
            }
        );
    }
}

fn expression_atom(expression: &sv_to_sexpr::ast::Expr) -> &str {
    match &expression.kind {
        sv_to_sexpr::ast::ExprKind::Path(path) => {
            assert_eq!(path.len(), 1);
            &path[0]
        }
        sv_to_sexpr::ast::ExprKind::Integer(value) | sv_to_sexpr::ast::ExprKind::Real(value) => {
            value
        }
        _ => panic!("expected scalar atom expression: {expression:?}"),
    }
}

fn source_targets(lowered: &LoweredModule) -> Vec<&str> {
    lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) if !assignment.target.starts_with('t') => {
                Some(assignment.target.as_str())
            }
            CellItem::Assignment(_) | CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect()
}

fn assert_flat_values(lowered: &LoweredModule) {
    for item in &lowered.cell.items {
        let CellItem::Assignment(assignment) = item else {
            continue;
        };
        if let Expr::List(items) = &assignment.expr {
            assert!(
                items
                    .iter()
                    .skip(1)
                    .all(|operand| matches!(operand, Expr::Atom(_))),
                "nested value operand in {}",
                assignment.target
            );
        }
    }
}

fn render_cli_ir(lowered: &LoweredModule) -> String {
    format!(
        "cell:\n{:#?}\ntiming aliases:\n{:#?}\n",
        lowered.cell, lowered.timing_aliases
    )
}

fn render_analysis(analysis: &sv_to_sexpr::analyze::AnalysisReport) -> String {
    normalize_repository_paths(analysis.render())
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
    normalize_repository_paths(output)
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
        .join("tests/fixtures/hierarchy")
        .join(format!("{name}.{extension}"))
}

fn fixture(name: &str, extension: &str) -> String {
    fs::read_to_string(fixture_path(name, extension)).unwrap()
}

fn assert_fixture(name: &str, extension: &str, actual: &str) {
    let path = fixture_path(name, extension);
    if std::env::var_os("UPDATE_HIERARCHY_GOLDENS").is_some() {
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "hierarchy fixture {} changed",
        path.display()
    );
}

fn temporary_output(name: &str, nodelay: bool) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sv-to-sexpr-hierarchy-{name}-{nodelay}-{}-{:?}.cell",
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
