use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sv_to_sexpr::analyze::{AnalysisReport, ModuleAnalysis, analyze_design_with_generate_mode};
use sv_to_sexpr::diagnostic::{Diagnostic, DiagnosticKind};
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, LoweredModule};
use sv_to_sexpr::lower::lower_file_with_generate_mode;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;

const DFFR_CC: &str = "sv-cells/dmg_cpu_b/cells/dffr_cc.sv";
const DFFR: &str = "sv-cells/dmg_cpu_b/cells/dffr.sv";

const DELAYFUL_DECLARATIONS: &[&str] = &["and1", "mux1", "mux1_buf", "mux2", "mux2_buf", "nand2"];
const NODELAY_DECLARATIONS: &[&str] = &["clk_buf", "clk_n_buf", "ff", "r_n_buf"];
const DELAYFUL_ALIASES: &[&str] = &[
    "T_fall_mux1",
    "T_fall_mux2",
    "T_fall_nand1",
    "T_fall_nand2",
    "T_fall_not1",
    "T_fall_not2",
    "T_fall_q",
    "T_fall_q_n",
    "T_rise_mux1",
    "T_rise_mux2",
    "T_rise_nand1",
    "T_rise_nand2",
    "T_rise_not1",
    "T_rise_not2",
    "T_rise_q",
    "T_rise_q_n",
];

struct ConfiguredResult {
    analysis: AnalysisReport,
    lowered: LoweredModule,
}

#[test]
fn dffr_cc_dual_mode_goldens_are_complete_deterministic_and_branch_exact() {
    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        let first = configured(DFFR_CC, mode);
        let second = configured(DFFR_CC, mode);
        assert_eq!(first.analysis, second.analysis);
        assert_eq!(first.lowered, second.lowered);
        first.lowered.cell.validate().unwrap();
        assert_flat_values(&first.lowered);
        assert_selected_dffr_cc(mode, &first.analysis.modules[0], &first.lowered);

        let prefix = format!("dffr_cc_{}", mode.label());
        assert_or_update_fixture(&prefix, "analysis", &first.analysis.render());
        assert_or_update_fixture(&prefix, "ir", &format!("{:#?}\n", first.lowered));
        assert_or_update_fixture(&prefix, "cell", &render_cell(&first.lowered.cell));
        assert_or_update_fixture(
            &prefix,
            "diagnostics",
            &render_diagnostics(&first.lowered.diagnostics),
        );
    }
}

#[test]
fn dffr_analysis_selects_continuous_or_procedural_buffer_drivers() {
    let delayful = configured(DFFR, GenerateMode::Delayful);
    let nodelay = configured(DFFR, GenerateMode::Nodelay);
    let delayful = &delayful.analysis.modules[0];
    let nodelay = &nodelay.analysis.modules[0];

    assert_eq!(
        targets(&delayful.continuous_assignments),
        ["clk_buf", "r_n_buf", "q_n"]
    );
    assert_eq!(targets(&delayful.procedural_assignments), ["q"]);
    assert_eq!(targets(&nodelay.continuous_assignments), ["q_n"]);
    assert_eq!(
        targets(&nodelay.procedural_assignments),
        ["clk_buf", "r_n_buf", "q"]
    );
    assert!(delayful.generate_alternatives.is_empty());
    assert!(nodelay.generate_alternatives.is_empty());
}

#[test]
fn cli_plumbs_generate_mode_through_convert_analyze_and_lower() {
    let root = repository_root();
    let input = root.join(DFFR_CC);
    let output = std::env::temp_dir().join(format!(
        "sv-to-sexpr-generate-{}-{:?}.cell",
        std::process::id(),
        std::thread::current().id()
    ));
    if output.exists() {
        fs::remove_file(&output).unwrap();
    }

    let delayful = run_cli(&[
        "convert-file",
        "--dry-run",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    let nodelay = run_cli(&[
        "convert-file",
        input.to_str().unwrap(),
        "--nodelay",
        output.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(delayful.status.success());
    assert!(nodelay.status.success());
    assert_ne!(delayful.stdout, nodelay.stdout);
    assert_eq!(
        String::from_utf8(delayful.stdout).unwrap(),
        fixture("dffr_cc_delayful", "cell")
    );
    assert_eq!(
        String::from_utf8(nodelay.stdout).unwrap(),
        fixture("dffr_cc_nodelay", "cell")
    );
    let delayful_stderr = String::from_utf8(delayful.stderr).unwrap();
    let nodelay_stderr = String::from_utf8(nodelay.stderr).unwrap();
    assert_eq!(delayful_stderr.matches(": intentional-ignore:").count(), 8);
    assert_eq!(nodelay_stderr.matches(": intentional-ignore:").count(), 2);
    assert_eq!(delayful_stderr.matches(": warning:").count(), 0);
    assert_eq!(nodelay_stderr.matches(": warning:").count(), 0);
    assert!(!output.exists());

    let analyze_delayful = run_cli(&["analyze", input.to_str().unwrap()]);
    let analyze_nodelay = run_cli(&["analyze", "--nodelay", input.to_str().unwrap()]);
    assert!(analyze_delayful.status.success());
    assert!(analyze_nodelay.status.success());
    let analyze_delayful = String::from_utf8(analyze_delayful.stdout).unwrap();
    let analyze_nodelay = String::from_utf8(analyze_nodelay.stdout).unwrap();
    assert!(analyze_delayful.contains("registers: mux1 mux2"));
    assert!(!analyze_delayful.contains("declaration ff "));
    assert!(analyze_nodelay.contains("registers: ff q"));
    assert!(!analyze_nodelay.contains("declaration mux1 "));

    let lower_delayful = run_cli(&["lower", input.to_str().unwrap()]);
    let lower_nodelay = run_cli(&["lower", input.to_str().unwrap(), "--nodelay"]);
    assert!(lower_delayful.status.success());
    assert!(lower_nodelay.status.success());
    let lower_delayful = String::from_utf8(lower_delayful.stdout).unwrap();
    let lower_nodelay = String::from_utf8(lower_nodelay.stdout).unwrap();
    assert!(lower_delayful.contains("registers: [\n        \"mux1\",\n        \"mux2\","));
    assert!(!lower_delayful.contains("\"ff\","));
    assert!(lower_nodelay.contains("registers: [\n        \"ff\",\n        \"q\","));
    assert!(!lower_nodelay.contains("\"mux1\","));
}

#[test]
fn generate_cell_goldens_are_canonical_for_sibling_formatter() {
    let root = repository_root();
    for name in ["dffr_cc_delayful", "dffr_cc_nodelay"] {
        let cell = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generate")
            .join(format!("{name}.cell"));
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
            "sexpr-fmt found non-canonical {name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
    }
}

fn configured(path: &str, mode: GenerateMode) -> ConfiguredResult {
    let input = fs::read_to_string(repository_root().join(path)).unwrap();
    let design = parse_file(Path::new(path), &input).unwrap();
    let analysis = analyze_design_with_generate_mode(&design, mode).unwrap();
    let lowered = lower_file_with_generate_mode(Path::new(path), &input, mode).unwrap();
    ConfiguredResult { analysis, lowered }
}

fn assert_selected_dffr_cc(mode: GenerateMode, module: &ModuleAnalysis, lowered: &LoweredModule) {
    assert!(module.generate_alternatives.is_empty());
    let declarations = module
        .declarations
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let aliases = lowered
        .timing_aliases
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let analysis_aliases = module
        .timing_aliases
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let driver_targets = module
        .drivers
        .iter()
        .map(|driver| driver.target.as_str())
        .collect::<Vec<_>>();
    let initial = targets(&module.initial_assignments);
    let procedural = targets(&module.procedural_assignments);
    let continuous = targets(&module.continuous_assignments);
    let diagnostics = &lowered.diagnostics;

    match mode {
        GenerateMode::Delayful => {
            assert_eq!(module.registers, ["mux1", "mux2"]);
            assert_eq!(declarations, DELAYFUL_DECLARATIONS);
            assert_eq!(aliases, DELAYFUL_ALIASES);
            assert_eq!(analysis_aliases, DELAYFUL_ALIASES);
            assert_eq!(initial, ["mux1", "mux2"]);
            assert_eq!(procedural, ["mux1", "mux2"]);
            assert_eq!(
                continuous,
                ["mux1_buf", "mux2_buf", "and1", "nand2", "q", "q_n"]
            );
            assert_eq!(
                diagnostic_count(diagnostics, DiagnosticKind::IntentionalIgnore),
                8
            );
            for absent in ["ff", "clk_buf", "clk_n_buf", "r_n_buf"] {
                assert!(!module.symbols.contains_key(absent));
                assert!(!lowered.timing_aliases.contains_key(absent));
                assert!(!source_targets(lowered).contains(&absent));
                assert!(!driver_targets.contains(&absent));
            }
        }
        GenerateMode::Nodelay => {
            assert_eq!(module.registers, ["ff", "q"]);
            assert_eq!(declarations, NODELAY_DECLARATIONS);
            assert!(aliases.is_empty());
            assert!(analysis_aliases.is_empty());
            assert_eq!(initial, ["ff", "q"]);
            assert_eq!(procedural, ["clk_buf", "clk_n_buf", "r_n_buf", "ff", "q"]);
            assert_eq!(continuous, ["q_n"]);
            assert_eq!(
                diagnostic_count(diagnostics, DiagnosticKind::IntentionalIgnore),
                2
            );
            for absent in ["and1", "nand2", "mux1", "mux2", "mux1_buf", "mux2_buf"] {
                assert!(!module.symbols.contains_key(absent));
                assert!(!lowered.timing_aliases.contains_key(absent));
                assert!(!source_targets(lowered).contains(&absent));
                assert!(!driver_targets.contains(&absent));
            }
            for alias in DELAYFUL_ALIASES {
                assert!(!module.localparams.contains_key(*alias));
                assert!(!lowered.timing_aliases.contains_key(*alias));
            }
        }
    }
    assert_eq!(diagnostic_count(diagnostics, DiagnosticKind::Warning), 0);
    assert_eq!(diagnostic_count(diagnostics, DiagnosticKind::Error), 0);
}

fn targets(assignments: &[sv_to_sexpr::analyze::AssignmentAnalysis]) -> Vec<&str> {
    assignments
        .iter()
        .map(|assignment| assignment.target.as_str())
        .collect()
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
        if let sv_to_sexpr::ir::Expr::List(items) = &assignment.expr {
            assert!(
                items
                    .iter()
                    .skip(1)
                    .all(|operand| matches!(operand, sv_to_sexpr::ir::Expr::Atom(_))),
                "nested value operand in {}",
                assignment.target
            );
        }
    }
}

fn diagnostic_count(diagnostics: &[Diagnostic], kind: DiagnosticKind) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == kind)
        .count()
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

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture(name: &str, extension: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generate")
            .join(format!("{name}.{extension}")),
    )
    .unwrap()
}

fn assert_or_update_fixture(name: &str, extension: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/generate")
        .join(format!("{name}.{extension}"));
    if std::env::var_os("UPDATE_GENERATE_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "generate fixture {} changed",
        path.display()
    );
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args(args)
        .output()
        .unwrap()
}
