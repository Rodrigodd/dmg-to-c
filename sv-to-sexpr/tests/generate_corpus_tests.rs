use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sv_to_sexpr::analyze::{
    AnalysisReport, ModuleAnalysis, ModuleCatalog, ScopeAnalysis, analyze_design_structural,
    analyze_design_with_catalog_and_generate_mode,
};
use sv_to_sexpr::ast::{Design, ExprKind, Item, ItemKind};
use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, LoweredModule};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::collect_sv_files;

const GENERATE_PATHS: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/dffr.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc_q.sv",
    "sv-cells/dmg_cpu_b/cells/dffsr.sv",
    "sv-cells/dmg_cpu_b/cells/tffnl.sv",
];
const TRANSISTOR_FAILURES: &[&str] = &[
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

struct Corpus {
    designs: BTreeMap<String, Design>,
    catalog: ModuleCatalog,
}

#[derive(Default)]
struct ModeTotals {
    analysis_supported: usize,
    analysis_deferred: usize,
    analysis_warned: usize,
    analysis_failed: usize,
    requirements: BTreeMap<String, usize>,
    lower_succeeded: usize,
    warnings: usize,
    intentional_ignores: usize,
    failures: BTreeMap<&'static str, Vec<String>>,
}

#[test]
fn dual_mode_generate_corpus_is_exact_selected_and_deterministic() {
    let corpus = load_corpus();
    assert_eq!(corpus.designs.len(), 206);
    let structural_generate = corpus
        .designs
        .iter()
        .filter_map(|(path, design)| has_generate(design).then_some(path.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(structural_generate, GENERATE_PATHS);

    let mut output =
        String::from("generate corpus audit\nfiles=206 generate-files=5 non-generate-files=201\n");
    output.push_str("structural-generate-forms:\n");
    for path in GENERATE_PATHS {
        render_structural_generate(path, &corpus.designs[*path], &mut output);
    }

    let mut mode_results = BTreeMap::new();
    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        let (totals, configured) = audit_mode(&corpus, mode, &mut output);
        mode_results.insert(mode.label(), configured);
        assert_mode_totals(&totals);
    }

    let delayful = &mode_results[GenerateMode::Delayful.label()];
    let nodelay = &mode_results[GenerateMode::Nodelay.label()];
    let mut identical_generate_cells = Vec::new();
    for path in GENERATE_PATHS {
        if render_cell(&delayful[*path].1.cell) == render_cell(&nodelay[*path].1.cell) {
            identical_generate_cells.push(*path);
        }
    }
    let mut identical_non_generate = 0;
    for (path, design) in &corpus.designs {
        if GENERATE_PATHS.contains(&path.as_str()) {
            continue;
        }
        let delayful_analysis = analyze_design_with_catalog_and_generate_mode(
            design,
            &corpus.catalog,
            GenerateMode::Delayful,
        )
        .unwrap();
        let nodelay_analysis = analyze_design_with_catalog_and_generate_mode(
            design,
            &corpus.catalog,
            GenerateMode::Nodelay,
        )
        .unwrap();
        assert_eq!(
            delayful_analysis, nodelay_analysis,
            "mode affected analysis {path}"
        );
        assert_eq!(
            lower_design_with_catalog_and_generate_mode(
                design,
                &corpus.catalog,
                GenerateMode::Delayful,
            ),
            lower_design_with_catalog_and_generate_mode(
                design,
                &corpus.catalog,
                GenerateMode::Nodelay,
            ),
            "mode affected lowering {path}"
        );
        identical_non_generate += 1;
    }
    assert_eq!(identical_non_generate, 201);
    assert_eq!(
        identical_generate_cells,
        [
            "sv-cells/dmg_cpu_b/cells/dffr.sv",
            "sv-cells/dmg_cpu_b/cells/dffsr.sv",
            "sv-cells/dmg_cpu_b/cells/tffnl.sv",
        ]
    );
    writeln!(
        &mut output,
        "mode-comparison: non-generate-identical=201 generate-cell-identical=[{}] generate-cell-distinct=[{},{}]",
        identical_generate_cells.join(","),
        GENERATE_PATHS[1],
        GENERATE_PATHS[2]
    )
    .unwrap();

    assert_or_update_fixture(&output);
}

#[test]
fn staged_cli_checks_report_exact_dual_mode_results() {
    let root = repository_root();
    let corpus = root.join("sv-cells");
    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        let mut analyze_args = vec!["check", corpus.to_str().unwrap(), "--stage", "analyze"];
        let mut lower_args = vec!["check", corpus.to_str().unwrap(), "--stage", "lower"];
        if mode == GenerateMode::Nodelay {
            analyze_args.push("--nodelay");
            lower_args.push("--nodelay");
        }
        let analyze = run_cli(&analyze_args);
        assert!(analyze.status.success());
        assert!(String::from_utf8(analyze.stdout).unwrap().starts_with(
            "analyze check summary: processed=206 supported=3 deferred=203 warned=0 failed=0\n"
        ));
        assert!(analyze.stderr.is_empty());

        let lower = run_cli(&lower_args);
        assert!(!lower.status.success());
        let expected_ignores = if mode == GenerateMode::Delayful {
            1108
        } else {
            1098
        };
        assert!(String::from_utf8(lower.stdout).unwrap().starts_with(&format!(
            "lower check summary: processed=206 warned=47 intentional-ignored={expected_ignores} failed=10\n"
        )));
        let stderr = String::from_utf8(lower.stderr).unwrap();
        assert!(stderr.ends_with("error: 10 files failed lowering\n"));
    }
}

fn audit_mode(
    corpus: &Corpus,
    mode: GenerateMode,
    output: &mut String,
) -> (
    ModeTotals,
    BTreeMap<String, (AnalysisReport, LoweredModule)>,
) {
    let mut totals = ModeTotals::default();
    let mut configured = BTreeMap::new();
    writeln!(output, "configured-mode {}:", mode.label()).unwrap();
    for (path, design) in &corpus.designs {
        let first_analysis =
            analyze_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode).unwrap();
        let second_analysis =
            analyze_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode).unwrap();
        assert_eq!(
            first_analysis, second_analysis,
            "analysis nondeterminism {path}"
        );
        assert_selected_analysis(path, &first_analysis);
        match first_analysis.disposition {
            sv_to_sexpr::analyze::AnalysisDisposition::Supported => totals.analysis_supported += 1,
            sv_to_sexpr::analyze::AnalysisDisposition::Deferred => totals.analysis_deferred += 1,
            sv_to_sexpr::analyze::AnalysisDisposition::Warned => totals.analysis_warned += 1,
            sv_to_sexpr::analyze::AnalysisDisposition::Failed => totals.analysis_failed += 1,
        }
        for requirement in &first_analysis.requirements {
            *totals
                .requirements
                .entry(requirement.capability_id.clone())
                .or_default() += 1;
        }

        let first_lower =
            lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode);
        let second_lower =
            lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode);
        assert_eq!(first_lower, second_lower, "lowering nondeterminism {path}");
        match first_lower {
            Ok(lowered) => {
                totals.lower_succeeded += 1;
                lowered.cell.validate().unwrap();
                totals.warnings += lowered
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
                    .count();
                totals.intentional_ignores += lowered
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
                    .count();
                if GENERATE_PATHS.contains(&path.as_str()) {
                    render_configured_signature(
                        path,
                        mode,
                        &first_analysis.modules[0],
                        &lowered,
                        output,
                    );
                }
                configured.insert(path.clone(), (first_analysis, lowered));
            }
            Err(diagnostic) => {
                let category = failure_category(path, &diagnostic.message);
                totals
                    .failures
                    .entry(category)
                    .or_default()
                    .push(path.clone());
            }
        }
    }
    writeln!(
        output,
        "  totals analysis-supported={} analysis-deferred={} analysis-warned={} analysis-failed={} lower-succeeded={} lower-failed={} warnings={} intentional-ignores={}",
        totals.analysis_supported,
        totals.analysis_deferred,
        totals.analysis_warned,
        totals.analysis_failed,
        totals.lower_succeeded,
        206 - totals.lower_succeeded,
        totals.warnings,
        totals.intentional_ignores
    )
    .unwrap();
    writeln!(
        output,
        "  requirements={}",
        render_map(&totals.requirements)
    )
    .unwrap();
    writeln!(
        output,
        "  failures-transistor=[{}]",
        totals
            .failures
            .get("transistor")
            .cloned()
            .unwrap_or_default()
            .join(",")
    )
    .unwrap();
    (totals, configured)
}

fn assert_mode_totals(totals: &ModeTotals) {
    assert_eq!(totals.analysis_supported, 3);
    assert_eq!(totals.analysis_deferred, 203);
    assert_eq!(totals.analysis_warned, 0);
    assert_eq!(totals.analysis_failed, 0);
    assert!(!totals.requirements.contains_key("generate.alternative"));
    assert_eq!(totals.lower_succeeded, 196);
    assert_eq!(totals.warnings, 47);
    assert!(matches!(totals.intentional_ignores, 1098 | 1108));
    assert_eq!(totals.failures["transistor"], TRANSISTOR_FAILURES);
}

fn assert_selected_analysis(path: &str, report: &AnalysisReport) {
    assert_eq!(report.modules.len(), 1);
    assert!(report.modules[0].generate_alternatives.is_empty(), "{path}");
    assert!(report.requirements.iter().all(|requirement| {
        requirement.capability_id != "generate.alternative"
            && requirement.milestone != sv_to_sexpr::analyze::TargetMilestone::M8GenerateSelection
    }));
    assert!(report.diagnostics.iter().all(|diagnostic| {
        !diagnostic.message.contains("generate branch")
            && !diagnostic.message.contains("generate condition")
    }));
}

fn render_structural_generate(path: &str, design: &Design, output: &mut String) {
    let module = design.first_module().unwrap();
    let generates = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Generate(block) => Some((item, block)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(generates.len(), 1, "{path}");
    let (generate_item, block) = generates[0];
    assert_eq!(block.items.len(), 1, "{path}");
    let ItemKind::If(statement) = &block.items[0].kind else {
        panic!("generate body is not if in {path}")
    };
    assert!(matches!(&statement.condition.kind, ExprKind::Path(parts) if parts == &["nodelay"]));
    let ItemKind::Block(then_block) = &statement.then_branch.kind else {
        panic!("then branch is not a block in {path}")
    };
    let else_branch = statement.else_branch.as_ref().expect("required else");
    let ItemKind::Block(else_block) = &else_branch.kind else {
        panic!("else branch is not a block in {path}")
    };

    let structural = analyze_design_structural(design);
    let alternative = &structural.modules[0].generate_alternatives[0];
    writeln!(
        output,
        "  {path} @{}:{} condition=nodelay then-items=[{}] else-items=[{}]",
        generate_item.span.line,
        generate_item.span.column,
        item_kinds(&then_block.items),
        item_kinds(&else_block.items)
    )
    .unwrap();
    writeln!(
        output,
        "    then {}",
        scope_signature(&alternative.then_branch)
    )
    .unwrap();
    writeln!(
        output,
        "    else {}",
        scope_signature(alternative.else_branch.as_ref().unwrap())
    )
    .unwrap();
}

fn render_configured_signature(
    path: &str,
    mode: GenerateMode,
    module: &ModuleAnalysis,
    lowered: &LoweredModule,
    output: &mut String,
) {
    let source_names = module.symbols.keys().collect::<BTreeSet<_>>();
    let source_targets = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) if source_names.contains(&assignment.target) => {
                Some(assignment.target.as_str())
            }
            CellItem::Assignment(_) | CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    writeln!(
        output,
        "  {path} {} declarations=[{}] registers=[{}] aliases=[{}] initial=[{}] procedural=[{}] continuous=[{}] drivers=[{}] source-targets=[{}] diagnostics=warning:{} ignore:{}",
        mode.label(),
        keys(&module.declarations),
        module.registers.join(","),
        keys(&module.timing_aliases),
        assignment_targets(&module.initial_assignments),
        assignment_targets(&module.procedural_assignments),
        assignment_targets(&module.continuous_assignments),
        module.drivers.iter().map(|driver| driver.target.as_str()).collect::<Vec<_>>().join(","),
        source_targets.join(","),
        lowered.diagnostics.iter().filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning).count(),
        lowered.diagnostics.iter().filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore).count(),
    )
    .unwrap();
}

fn scope_signature(scope: &ScopeAnalysis) -> String {
    format!(
        "declarations=[{}] registers=[{}] aliases=[{}] initial=[{}] procedural=[{}] continuous=[{}] drivers=[{}]",
        keys(&scope.declarations),
        scope.registers.join(","),
        keys(&scope.timing_aliases),
        assignment_targets(&scope.initial_assignments),
        assignment_targets(&scope.procedural_assignments),
        assignment_targets(&scope.continuous_assignments),
        scope
            .drivers
            .iter()
            .map(|driver| driver.target.as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn item_kinds(items: &[Item]) -> String {
    items.iter().map(item_kind).collect::<Vec<_>>().join(",")
}

fn item_kind(item: &Item) -> &'static str {
    match &item.kind {
        ItemKind::Import(_) => "import",
        ItemKind::Decl(_) => "decl",
        ItemKind::Initial(_) => "initial",
        ItemKind::ProcAssign(_) => "proc-assign",
        ItemKind::AlwaysLatch(_) => "always-latch",
        ItemKind::Always(_) => "always",
        ItemKind::Assign(_) => "assign",
        ItemKind::Primitive(_) => "primitive",
        ItemKind::Instantiation(_) => "instantiation",
        ItemKind::Specify(_) => "specify",
        ItemKind::Generate(_) => "generate",
        ItemKind::Block(_) => "block",
        ItemKind::If(_) => "if",
        ItemKind::Empty => "empty",
    }
}

fn assignment_targets(assignments: &[sv_to_sexpr::analyze::AssignmentAnalysis]) -> String {
    assignments
        .iter()
        .map(|assignment| assignment.target.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn keys<T>(map: &BTreeMap<String, T>) -> String {
    map.keys().map(String::as_str).collect::<Vec<_>>().join(",")
}

fn has_generate(design: &Design) -> bool {
    design.modules().any(|module| {
        module
            .items
            .iter()
            .any(|item| matches!(item.kind, ItemKind::Generate(_)))
    })
}

fn failure_category(path: &str, message: &str) -> &'static str {
    if TRANSISTOR_FAILURES.contains(&path) {
        assert!(message.starts_with("unsupported primitive"));
        "transistor"
    } else {
        panic!("unexpected configured failure {path}: {message}")
    }
}

fn render_map(values: &BTreeMap<String, usize>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}:{value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn load_corpus() -> Corpus {
    let root = repository_root();
    let mut designs = BTreeMap::new();
    for source in collect_sv_files(&root.join("sv-cells")).unwrap() {
        let path = source
            .strip_prefix(&root)
            .unwrap()
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let input = fs::read_to_string(&source).unwrap();
        let design = parse_file(Path::new(&path), &input).unwrap();
        designs.insert(path, design);
    }
    let catalog =
        ModuleCatalog::from_designs(&designs.values().cloned().collect::<Vec<_>>()).unwrap();
    Corpus { designs, catalog }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn assert_or_update_fixture(actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/generate/corpus_summary.generate");
    if std::env::var_os("UPDATE_GENERATE_CORPUS_GOLDEN").is_some() {
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(actual, expected, "generate corpus summary changed");
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args(args)
        .output()
        .unwrap()
}
