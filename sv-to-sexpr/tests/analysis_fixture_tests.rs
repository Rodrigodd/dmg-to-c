mod analysis_support;

use std::collections::{BTreeMap, BTreeSet};

use analysis_support::{assert_or_update_fixture, corpus};
use sv_to_sexpr::analyze::{
    AnalysisDisposition, DriverSource, InstantiationResolution, SignalRole, SymbolCategory,
    TargetMilestone,
};

#[test]
fn representative_analysis_goldens_are_deterministic_and_semantically_complete() {
    let corpus = corpus();
    let fixtures = [
        (
            "reference.analysis",
            "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        ),
        (
            "combinational_internal.analysis",
            "sv-cells/sm83/cells/alu_cgen.sv",
        ),
        ("primitive_tri.analysis", "sv-cells/dmg_cpu_b/cells/muxi.sv"),
        (
            "generated_dff.analysis",
            "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
        ),
        (
            "hierarchical_adder.analysis",
            "sv-cells/dmg_cpu_b/cells/full_add.sv",
        ),
    ];
    for (fixture, source) in fixtures {
        let report = if fixture == "generated_dff.analysis" {
            corpus.analyze_structural(source)
        } else {
            corpus.analyze(source)
        };
        let rendered = report.render();
        assert_eq!(rendered, report.render());
        assert!(rendered.contains(source));
        assert!(!rendered.contains(&corpus.repository_root.to_string_lossy().to_string()));
        assert_or_update_fixture(fixture, &rendered);
    }

    let reference = corpus.analyze("sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv");
    let module = &reference.modules[0];
    assert_eq!(
        module.inputs,
        vec!["clk", "clk_n", "ena", "ena_n", "s_n", "pch_n", "d"]
    );
    assert_eq!(module.outputs, vec!["q", "q_n", "d"]);
    assert_eq!(module.registers, vec!["ff1", "ff2", "q_n"]);
    assert_eq!(module.symbols["d"].category, SymbolCategory::Port);
    assert_eq!(module.symbols["ff1"].category, SymbolCategory::Declaration);
    assert_eq!(module.specify_paths.len(), 2);
    assert_eq!(module.timing_aliases.len(), 10);
    assert!(module.drivers.iter().any(|driver| {
        driver.target == "d"
            && matches!(
                &driver.source,
                DriverSource::Primitive { name } if name == "bufif0"
            )
    }));

    let combinational = corpus.analyze("sv-cells/sm83/cells/alu_cgen.sv");
    let module = &combinational.modules[0];
    assert!(module.registers.is_empty());
    assert!(
        module.signal_roles["cout0_n_p"]
            .roles
            .contains(&SignalRole::ContinuousDriven)
    );
    assert!(
        module
            .signal_roles
            .values()
            .all(|signal| !signal.roles.contains(&SignalRole::ModeledState))
    );

    let primitive = corpus.analyze("sv-cells/dmg_cpu_b/cells/muxi.sv");
    let module = &primitive.modules[0];
    assert!(module.registers.is_empty());
    assert!(
        module.signal_roles["mux"]
            .roles
            .contains(&SignalRole::PrimitiveDriven)
    );
    assert!(
        module.signal_roles["mux"]
            .roles
            .contains(&SignalRole::HierarchicalConnection)
    );
    assert!(
        !module.signal_roles["mux"]
            .roles
            .contains(&SignalRole::ModeledState)
    );
    assert_eq!(
        module
            .drivers
            .iter()
            .filter(|driver| {
                driver.target == "mux" && matches!(driver.source, DriverSource::Primitive { .. })
            })
            .count(),
        4
    );

    let generated = corpus.analyze_structural("sv-cells/dmg_cpu_b/cells/dffr_cc.sv");
    let module = &generated.modules[0];
    assert!(module.registers.is_empty());
    assert!(module.drivers.is_empty());
    assert_eq!(module.generate_alternatives.len(), 1);
    let alternative = &module.generate_alternatives[0];
    assert_eq!(alternative.condition.text, "nodelay");
    assert_eq!(alternative.then_branch.registers, vec!["ff", "q"]);
    assert_eq!(
        alternative.else_branch.as_ref().unwrap().registers,
        vec!["mux1", "mux2"]
    );
    assert!(alternative.then_branch.symbols.contains_key("ff"));
    assert!(
        alternative
            .else_branch
            .as_ref()
            .unwrap()
            .symbols
            .contains_key("mux1")
    );

    let hierarchy = corpus.analyze("sv-cells/dmg_cpu_b/cells/full_add.sv");
    let module = &hierarchy.modules[0];
    assert_eq!(module.instantiations.len(), 5);
    assert!(module.registers.is_empty());
    assert!(module.continuous_assignments.is_empty());
    assert!(module.procedural_assignments.is_empty());
    assert!(module.instantiations.iter().all(|instantiation| {
        matches!(
            instantiation.resolution,
            InstantiationResolution::Resolved(_)
        )
    }));
    assert_eq!(
        module
            .drivers
            .iter()
            .map(|driver| driver.target.as_str())
            .collect::<Vec<_>>(),
        vec!["sum", "caxb", "cout", "ab", "axb"]
    );
}

#[test]
fn corpus_analysis_summary_is_stable_by_disposition_milestone_and_capability() {
    let corpus = corpus();
    let mut disposition_counts = BTreeMap::<AnalysisDisposition, usize>::new();
    let mut milestone_files = BTreeMap::<TargetMilestone, BTreeSet<String>>::new();
    let mut capability_files = BTreeMap::<(String, TargetMilestone), BTreeSet<String>>::new();
    for path in corpus.designs.keys() {
        let report = corpus.analyze(path);
        *disposition_counts.entry(report.disposition).or_default() += 1;
        for requirement in report.requirements {
            milestone_files
                .entry(requirement.milestone)
                .or_default()
                .insert(path.clone());
            capability_files
                .entry((requirement.capability_id, requirement.milestone))
                .or_default()
                .insert(path.clone());
        }
    }
    assert_eq!(corpus.designs.len(), 206);
    assert_eq!(
        disposition_counts
            .get(&AnalysisDisposition::Supported)
            .copied()
            .unwrap_or_default(),
        1
    );
    assert_eq!(
        disposition_counts
            .get(&AnalysisDisposition::Deferred)
            .copied()
            .unwrap_or_default(),
        205
    );
    assert_eq!(
        disposition_counts
            .get(&AnalysisDisposition::Failed)
            .copied()
            .unwrap_or_default(),
        0
    );

    let mut rendered = String::new();
    rendered.push_str("analysis corpus summary\n");
    rendered.push_str(&format!(
        "processed=206 supported={} deferred={} warned={} failed={}\n",
        disposition_counts
            .get(&AnalysisDisposition::Supported)
            .copied()
            .unwrap_or_default(),
        disposition_counts
            .get(&AnalysisDisposition::Deferred)
            .copied()
            .unwrap_or_default(),
        disposition_counts
            .get(&AnalysisDisposition::Warned)
            .copied()
            .unwrap_or_default(),
        disposition_counts
            .get(&AnalysisDisposition::Failed)
            .copied()
            .unwrap_or_default()
    ));
    rendered.push_str("milestones:\n");
    for (milestone, files) in &milestone_files {
        rendered.push_str(&format!("  {} files={}\n", milestone.label(), files.len()));
    }
    rendered.push_str("capabilities:\n");
    for ((capability, milestone), files) in &capability_files {
        rendered.push_str(&format!(
            "  {} | {} | files={}\n",
            capability,
            milestone.label(),
            files.len()
        ));
    }
    assert_eq!(rendered, rendered.clone());
    assert_or_update_fixture("corpus_summary.analysis", &rendered);
}
