#[allow(dead_code)]
mod analysis_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use analysis_support::corpus;
use sv_to_sexpr::analyze::{
    AnalysisDisposition, DriverSource, InstantiationResolution, SignalRole, TargetMilestone,
    analyze_design_with_catalog_and_generate_mode,
};
use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, Expr, ValueOperator};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;

const FAILURES: &[&str] = &[
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeeperRow {
    path: String,
    instance: String,
    target: String,
    source_order: usize,
    line: usize,
    column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmittedRow {
    path: String,
    target: String,
    target_driver_ordinal: usize,
    target_driver_count: usize,
    assignment_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModeAudit {
    mode: GenerateMode,
    supported: usize,
    deferred: usize,
    keepers: Vec<KeeperRow>,
    emitted: Vec<EmittedRow>,
    keeper_deferrals: Vec<String>,
    successes: usize,
    failures: BTreeMap<String, String>,
    warnings: usize,
    ignores: usize,
}

#[test]
fn configured_keeper_corpus_is_exact_distinct_and_m11_only() {
    let delayful = audit_mode(GenerateMode::Delayful);
    let nodelay = audit_mode(GenerateMode::Nodelay);
    assert_eq!(delayful.keepers, nodelay.keepers);
    assert_eq!(delayful.emitted, nodelay.emitted);
    assert_eq!(delayful.failures, nodelay.failures);
    assert_eq!(delayful.successes, 196);
    assert_eq!(nodelay.successes, 196);
    assert_eq!(delayful.failures.len(), 10);
    assert_eq!(nodelay.failures.len(), 10);
    assert_eq!(delayful.warnings, 47);
    assert_eq!(nodelay.warnings, 47);
    assert_eq!(delayful.ignores, 1108);
    assert_eq!(nodelay.ignores, 1098);
    assert_eq!(delayful.supported, 3);
    assert_eq!(delayful.deferred, 203);
    assert_eq!(nodelay.supported, 3);
    assert_eq!(nodelay.deferred, 203);
    assert_eq!(
        delayful
            .failures
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        FAILURES
    );
    assert!(delayful.failures.values().all(|diagnostic| {
        diagnostic.contains("unsupported primitive nmos")
            || diagnostic.contains("unsupported primitive pmos")
            || diagnostic.contains("unsupported primitive rnmos")
    }));
    assert_eq!(
        delayful.keeper_deferrals,
        [
            "sv-cells/sm83/cells/dlatch_ee_irq.sv | rnmos | 23:2",
            "sv-cells/sm83/cells/idu_bit0.sv | nmos | 37:2",
        ]
    );
    assert_eq!(delayful.keepers.len(), 6);
    assert_eq!(delayful.emitted.len(), 4);

    let summary = format!("{}{}", render_audit(&delayful), render_audit(&nodelay));
    assert_or_update_fixture(&summary);
}

fn audit_mode(mode: GenerateMode) -> ModeAudit {
    let corpus = corpus();
    assert_eq!(corpus.designs.len(), 206);
    let mut audit = ModeAudit {
        mode,
        supported: 0,
        deferred: 0,
        keepers: Vec::new(),
        emitted: Vec::new(),
        keeper_deferrals: Vec::new(),
        successes: 0,
        failures: BTreeMap::new(),
        warnings: 0,
        ignores: 0,
    };

    for (path, design) in &corpus.designs {
        let analysis = analyze_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode)
            .unwrap_or_else(|error| panic!("analysis failed for {path}: {error}"));
        match analysis.disposition {
            AnalysisDisposition::Supported => audit.supported += 1,
            AnalysisDisposition::Deferred => audit.deferred += 1,
            other => panic!("unexpected configured disposition for {path}: {other:?}"),
        }
        assert!(analysis.requirements.iter().all(|requirement| {
            requirement.capability_id != "hierarchy.keeper"
                && requirement.milestone != TargetMilestone::M10Keeper
        }));
        let module = &analysis.modules[0];
        let mut file_keeper = None;
        for instantiation in &module.instantiations {
            if instantiation.module != "keeper" {
                continue;
            }
            let InstantiationResolution::Special(special) = &instantiation.resolution else {
                panic!("unresolved keeper in {path}")
            };
            assert!(file_keeper.is_none(), "multiple keepers in {path}");
            let keeper = &special.keeper;
            let target = &keeper.connection.target;
            assert!(
                module.signal_roles[target]
                    .roles
                    .contains(&SignalRole::KeeperDriven)
            );
            assert!(!module.registers.iter().any(|register| register == target));
            let keeper_drivers = module
                .drivers
                .iter()
                .filter(|driver| {
                    driver.target == *target
                        && matches!(
                            &driver.source,
                            DriverSource::Keeper { instance } if instance == &keeper.instance
                        )
                })
                .count();
            assert_eq!(keeper_drivers, 1, "{path}");
            let row = KeeperRow {
                path: path.clone(),
                instance: keeper.instance.clone(),
                target: target.clone(),
                source_order: instantiation.source_order,
                line: instantiation.span.line,
                column: instantiation.span.column,
            };
            file_keeper = Some(row.clone());
            audit.keepers.push(row);
        }

        let first = lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode);
        let second = lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode);
        assert_eq!(first, second, "nondeterministic lowering for {path}");
        match first {
            Ok(lowered) => {
                audit.successes += 1;
                lowered.cell.validate().unwrap();
                audit.warnings += lowered
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
                    .count();
                audit.ignores += lowered
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
                    .count();
                if let Some(keeper) = file_keeper {
                    let target_drivers = module
                        .drivers
                        .iter()
                        .filter(|driver| driver.target == keeper.target)
                        .collect::<Vec<_>>();
                    let target_driver_ordinal = target_drivers
                        .iter()
                        .position(|driver| matches!(driver.source, DriverSource::Keeper { .. }))
                        .unwrap();
                    let target_assignments = lowered
                        .cell
                        .items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| match item {
                            CellItem::Assignment(assignment)
                                if assignment.target == keeper.target =>
                            {
                                Some((index, assignment))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(target_assignments.len(), target_drivers.len(), "{path}");
                    let keeper_assignments = target_assignments
                        .iter()
                        .enumerate()
                        .filter(|(_, (_, assignment))| {
                            assignment.expr == Expr::value(ValueOperator::Keeper, vec![])
                                && assignment.delay == Expr::atom("0")
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(keeper_assignments.len(), 1, "{path}");
                    assert_eq!(keeper_assignments[0].0, target_driver_ordinal, "{path}");
                    audit.emitted.push(EmittedRow {
                        path: path.clone(),
                        target: keeper.target,
                        target_driver_ordinal,
                        target_driver_count: target_drivers.len(),
                        assignment_index: keeper_assignments[0].1.0,
                    });
                }
            }
            Err(error) => {
                let logical = format!(
                    "{}:{}:{}: {}",
                    path, error.span.line, error.span.column, error.message
                );
                audit.failures.insert(path.clone(), logical);
                if file_keeper.is_some() {
                    let primitive = error
                        .message
                        .strip_prefix("unsupported primitive ")
                        .unwrap();
                    audit.keeper_deferrals.push(format!(
                        "{path} | {primitive} | {}:{}",
                        error.span.line, error.span.column
                    ));
                }
            }
        }
    }

    audit
        .keepers
        .sort_by(|left, right| left.path.cmp(&right.path));
    audit
        .emitted
        .sort_by(|left, right| left.path.cmp(&right.path));
    audit.keeper_deferrals.sort();
    assert_eq!(audit.successes + audit.failures.len(), 206);
    assert_eq!(
        audit
            .keepers
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        6
    );
    audit
}

fn render_audit(audit: &ModeAudit) -> String {
    let mut output = String::new();
    writeln!(&mut output, "keeper corpus mode={}", audit.mode.label()).unwrap();
    writeln!(
        &mut output,
        "analysis supported={} deferred={} keeper-requirements=0",
        audit.supported, audit.deferred
    )
    .unwrap();
    writeln!(
        &mut output,
        "lower successes={} failures={} warnings={} intentional-ignores={}",
        audit.successes,
        audit.failures.len(),
        audit.warnings,
        audit.ignores
    )
    .unwrap();
    writeln!(&mut output, "resolved-keepers:").unwrap();
    for row in &audit.keepers {
        writeln!(
            &mut output,
            "  {} | {} | {} | source-order={} | {}:{}",
            row.path, row.instance, row.target, row.source_order, row.line, row.column
        )
        .unwrap();
    }
    writeln!(&mut output, "emitted-keepers:").unwrap();
    for row in &audit.emitted {
        writeln!(
            &mut output,
            "  {} | {} | target-ordinal={}/{} | assignment-index={} | value=(keeper) | delay=0",
            row.path,
            row.target,
            row.target_driver_ordinal,
            row.target_driver_count,
            row.assignment_index
        )
        .unwrap();
    }
    writeln!(&mut output, "keeper-bearing-m11-deferrals:").unwrap();
    for row in &audit.keeper_deferrals {
        writeln!(&mut output, "  {row}").unwrap();
    }
    writeln!(&mut output, "failures:").unwrap();
    for diagnostic in audit.failures.values() {
        writeln!(&mut output, "  {diagnostic}").unwrap();
    }
    output
}

fn assert_or_update_fixture(actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/keeper/corpus_summary.keeper");
    if std::env::var_os("UPDATE_KEEPER_CORPUS_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(actual, expected, "keeper corpus fixture changed");
}
