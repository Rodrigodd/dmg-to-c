#[allow(dead_code)]
mod analysis_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use analysis_support::corpus;
use sv_to_sexpr::analyze::{
    AnalysisDisposition, ConnectionSource, InstantiationResolution, ParameterBindingSource,
    TargetMilestone,
};
use sv_to_sexpr::ast::Direction;
use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, Expr, LoweredModule};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;

const HALF_ADD: &str = "sv-cells/dmg_cpu_b/cells/half_add.sv";
const FULL_ADD: &str = "sv-cells/dmg_cpu_b/cells/full_add.sv";
const KEEPER_FAILURES: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/mux.sv",
    "sv-cells/dmg_cpu_b/cells/muxi.sv",
    "sv-cells/dmg_cpu_b/cells/pad_xtal.sv",
    "sv-cells/sm83/cells/idu_bit0.sv",
    "sv-cells/sm83/cells/reg_wz_out.sv",
];
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

#[test]
fn configured_hierarchy_corpus_is_exact_resolved_and_fully_lowered() {
    let corpus = corpus();
    assert_eq!(corpus.designs.len(), 206);
    let mut output = String::from("hierarchy corpus audit\nfiles=206\n");

    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        let mut ordinary_files = BTreeSet::new();
        let mut module_counts = BTreeMap::<String, usize>::new();
        let mut bindings = 0;
        let mut connections = 0;
        let mut input_connections = 0;
        let mut output_connections = 0;
        let mut supported = 0;
        let mut deferred = 0;
        let mut lower_succeeded = 0;
        let mut warnings = 0;
        let mut ignores = 0;
        let mut keeper_failures = Vec::new();
        let mut transistor_failures = Vec::new();

        for (path, design) in &corpus.designs {
            let analysis = sv_to_sexpr::analyze::analyze_design_with_catalog_and_generate_mode(
                design,
                &corpus.catalog,
                mode,
            )
            .unwrap();
            assert!(analysis.requirements.iter().all(|requirement| {
                requirement.milestone != TargetMilestone::M9OrdinaryHierarchy
                    && requirement.capability_id != "hierarchy.ordinary"
            }));
            match analysis.disposition {
                AnalysisDisposition::Supported => supported += 1,
                AnalysisDisposition::Deferred => deferred += 1,
                other => panic!("unexpected configured analysis disposition {path}: {other:?}"),
            }
            for instance in &analysis.modules[0].instantiations {
                match &instance.resolution {
                    InstantiationResolution::Resolved(resolved) => {
                        ordinary_files.insert(path.clone());
                        *module_counts.entry(instance.module.clone()).or_default() += 1;
                        assert_eq!(resolved.parameter_bindings.len(), 1);
                        let binding = &resolved.parameter_bindings[0];
                        assert_eq!(binding.parameter, "L_y");
                        assert_eq!(binding.source, ParameterBindingSource::Named);
                        bindings += 1;
                        for connection in &resolved.connections {
                            assert_eq!(connection.source, ConnectionSource::Named);
                            connections += 1;
                            match connection.direction {
                                Direction::Input => input_connections += 1,
                                Direction::Output => output_connections += 1,
                                Direction::Inout => panic!("ordinary adder inout connection"),
                            }
                        }
                    }
                    InstantiationResolution::Special(_) => {}
                    InstantiationResolution::Unresolved => {
                        panic!(
                            "configured unresolved instance {path}: {}",
                            instance.instance
                        )
                    }
                }
            }
            if path == HALF_ADD {
                assert_eq!(
                    instance_signatures(&analysis.modules[0].instantiations),
                    [
                        "and2_cout_inst:dmg_and2:L_cout:y=cout,in1=a,in2=b",
                        "xor_sum_inst:dmg_xor:L_sum:y=sum,in1=a,in2=b",
                    ]
                );
            } else if path == FULL_ADD {
                assert_eq!(
                    instance_signatures(&analysis.modules[0].instantiations),
                    [
                        "xor_sum_inst:dmg_xor:L_sum:y=sum,in1=axb,in2=cin",
                        "nand2_caxb_inst:dmg_nand2:120:y=caxb,in1=cin,in2=axb",
                        "nand2_cout_inst:dmg_nand2:L_cout:y=cout,in1=ab,in2=caxb",
                        "nand2_ab_inst:dmg_nand2:119:y=ab,in1=b,in2=a",
                        "xor_axb_inst:dmg_xor:296:y=axb,in1=a,in2=b",
                    ]
                );
            }

            match lower_design_with_catalog_and_generate_mode(design, &corpus.catalog, mode) {
                Ok(lowered) => {
                    lower_succeeded += 1;
                    warnings += diagnostic_count(&lowered, DiagnosticKind::Warning);
                    ignores += diagnostic_count(&lowered, DiagnosticKind::IntentionalIgnore);
                    if matches!(path.as_str(), HALF_ADD | FULL_ADD) {
                        assert_adder(path, &lowered);
                    }
                }
                Err(diagnostic) if KEEPER_FAILURES.contains(&path.as_str()) => {
                    assert!(
                        diagnostic
                            .message
                            .starts_with("unsupported item for lowering")
                    );
                    keeper_failures.push(path.clone());
                }
                Err(diagnostic) if TRANSISTOR_FAILURES.contains(&path.as_str()) => {
                    assert!(diagnostic.message.starts_with("unsupported primitive"));
                    transistor_failures.push(path.clone());
                }
                Err(diagnostic) => panic!("unexpected configured failure {path}: {diagnostic}"),
            }
        }

        assert_eq!(
            ordinary_files,
            BTreeSet::from([FULL_ADD.into(), HALF_ADD.into()])
        );
        assert_eq!(
            module_counts,
            BTreeMap::from([
                ("dmg_and2".into(), 1),
                ("dmg_nand2".into(), 3),
                ("dmg_xor".into(), 3),
            ])
        );
        assert_eq!(bindings, 7);
        assert_eq!(connections, 21);
        assert_eq!(input_connections, 14);
        assert_eq!(output_connections, 7);
        assert_eq!((supported, deferred), (3, 203));
        assert_eq!(lower_succeeded, 192);
        assert_eq!(warnings, 47);
        assert_eq!(
            ignores,
            match mode {
                GenerateMode::Delayful => 1073,
                GenerateMode::Nodelay => 1063,
            }
        );
        assert_eq!(keeper_failures, KEEPER_FAILURES);
        assert_eq!(transistor_failures, TRANSISTOR_FAILURES);

        writeln!(
            &mut output,
            "mode={} analysis-supported={supported} analysis-deferred={deferred} ordinary-files=[{}] ordinary-instances={bindings} modules={} bindings=named-L_y:{bindings},positional:0 connections=named:{connections},positional:0,input:{input_connections},output:{output_connections},inout:0 lower-succeeded={lower_succeeded} lower-failed={} warnings={warnings} intentional-ignores={ignores}",
            mode.label(),
            ordinary_files.into_iter().collect::<Vec<_>>().join(","),
            render_counts(&module_counts),
            keeper_failures.len() + transistor_failures.len(),
        )
        .unwrap();
        writeln!(
            &mut output,
            "  keeper-failures=[{}]",
            keeper_failures.join(",")
        )
        .unwrap();
        writeln!(
            &mut output,
            "  transistor-failures=[{}]",
            transistor_failures.join(",")
        )
        .unwrap();
    }
    writeln!(
        &mut output,
        "half_add instances=[and2_cout_inst:dmg_and2:L_cout:y=cout,in1=a,in2=b,xor_sum_inst:dmg_xor:L_sum:y=sum,in1=a,in2=b] targets=[cout,sum] aliases=8"
    )
    .unwrap();
    writeln!(
        &mut output,
        "full_add instances=[xor_sum_inst:dmg_xor:L_sum:y=sum,in1=axb,in2=cin,nand2_caxb_inst:dmg_nand2:120:y=caxb,in1=cin,in2=axb,nand2_cout_inst:dmg_nand2:L_cout:y=cout,in1=ab,in2=caxb,nand2_ab_inst:dmg_nand2:119:y=ab,in1=b,in2=a,xor_axb_inst:dmg_xor:296:y=axb,in1=a,in2=b] targets=[sum,caxb,cout,ab,axb] aliases=14"
    )
    .unwrap();

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hierarchy/corpus_summary.hierarchy");
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(output, expected, "hierarchy corpus summary changed");
}

fn assert_adder(path: &str, lowered: &LoweredModule) {
    lowered.cell.validate().unwrap();
    assert!(lowered.cell.registers.is_empty());
    let assignments = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(
        assignments
            .iter()
            .all(|assignment| !assignment.target.starts_with('t'))
    );
    assert!(assignments.iter().all(|assignment| {
        match &assignment.expr {
            Expr::Atom(_) => true,
            Expr::List(items) => items
                .iter()
                .skip(1)
                .all(|item| matches!(item, Expr::Atom(_))),
        }
    }));
    let expected = if path == HALF_ADD {
        vec!["cout", "sum"]
    } else {
        vec!["sum", "caxb", "cout", "ab", "axb"]
    };
    assert_eq!(
        assignments
            .iter()
            .map(|assignment| assignment.target.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        lowered.timing_aliases.len(),
        if path == HALF_ADD { 8 } else { 14 }
    );
    assert!(
        lowered
            .timing_aliases
            .keys()
            .all(|alias| alias.contains("__"))
    );
    let debug = format!("{lowered:#?}");
    for leaked in ["L_y", "\"in1\"", "\"in2\"", "\"y\""] {
        assert!(
            !debug.contains(leaked),
            "{path} leaked child symbol {leaked}"
        );
    }
}

fn diagnostic_count(lowered: &LoweredModule, kind: DiagnosticKind) -> usize {
    lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == kind)
        .count()
}

fn instance_signatures(instances: &[sv_to_sexpr::analyze::InstantiationAnalysis]) -> Vec<String> {
    instances
        .iter()
        .map(|instance| {
            let InstantiationResolution::Resolved(resolved) = &instance.resolution else {
                panic!("expected resolved ordinary instance")
            };
            let binding = expression_atom(&resolved.parameter_bindings[0].expression);
            let connections = resolved
                .connections
                .iter()
                .map(|connection| {
                    format!(
                        "{}={}",
                        connection.port,
                        expression_atom(&connection.expression)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}:{}:{binding}:{connections}",
                instance.instance, instance.module
            )
        })
        .collect()
}

fn expression_atom(expression: &sv_to_sexpr::ast::Expr) -> &str {
    match &expression.kind {
        sv_to_sexpr::ast::ExprKind::Path(path) => &path[0],
        sv_to_sexpr::ast::ExprKind::Integer(value) | sv_to_sexpr::ast::ExprKind::Real(value) => {
            value
        }
        other => panic!("non-atom hierarchy expression {other:?}"),
    }
}

fn render_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}
