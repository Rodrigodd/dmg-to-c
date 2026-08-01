use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::analyze::ModuleCatalog;
use sv_to_sexpr::ast::Design;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::LoweredModule;
use sv_to_sexpr::lower::{
    lower_design_with_catalog_and_generate_mode,
    lower_design_with_timing_and_catalog_and_generate_mode,
};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::survey::collect_sv_files;
use sv_to_sexpr::timing_graph::{
    AssignmentProvenance, ControlGroupKind, DependencyKind, PublicOutputSplit, TargetGroupKind,
    TimingAnalysisReport, TimingConstraint, TimingNodeKind, TimingPathSense, TimingSense,
    Transition, TransitionEffect,
};

const TFFNL: &str = "sv-cells/dmg_cpu_b/cells/tffnl.sv";
const AO21: &str = "sv-cells/dmg_cpu_b/cells/ao21.sv";
const HALF_ADD: &str = "sv-cells/dmg_cpu_b/cells/half_add.sv";
const B2B_WAND: &str = "sv-cells/sm83/cells/b2b_wand_inj_a.sv";

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    lowered: LoweredModule,
    assignment_provenance: Vec<AssignmentProvenance>,
    report: TimingAnalysisReport,
    rendered_report: String,
}

#[derive(Debug, Default)]
struct ModeAudit {
    files: usize,
    source_constraints: usize,
    source_controls: usize,
    source_target_groups: usize,
    source_multiple_target_groups: usize,
    source_tuple_arities: [usize; 4],
    elaborated_constraints: usize,
    elaborated_controls: usize,
    elaborated_target_groups: usize,
    elaborated_tuple_arities: [usize; 4],
    foreign_constraints: usize,
    expanded_only_roots: BTreeSet<String>,
    nodes: usize,
    signals: usize,
    assignments: usize,
    dependencies: usize,
    dependency_kinds: BTreeMap<String, usize>,
    dependency_senses: BTreeMap<String, usize>,
    state_control_transitions: BTreeMap<String, usize>,
    state_boundaries: usize,
    resolved_net_boundaries: usize,
    constraints: usize,
    controls: usize,
    control_groups: usize,
    control_group_kinds: BTreeMap<String, usize>,
    target_groups: usize,
    target_group_kinds: BTreeMap<String, usize>,
    path_senses: BTreeMap<String, usize>,
    public_splits: BTreeMap<String, usize>,
    nonempty_prefix_groups: usize,
    prefix_nodes: usize,
    nonempty_suffix_groups: usize,
    suffix_nodes: usize,
    nonempty_reconvergent_groups: usize,
    reconvergent_nodes: usize,
    multiple_witnesses: BTreeMap<String, String>,
    combinational_cycle_errors: usize,
    unreachable_control_errors: usize,
    additive_rebuild_failures: usize,
    nondeterministic_reports: usize,
    ordinary_compatibility_mismatches: usize,
    absolute_path_leaks: usize,
}

#[test]
fn complete_timing_graph_corpus_is_exact_owned_deterministic_and_compatible() {
    let root = repository_root();
    let entries = parse_sorted_corpus(&root);
    assert_eq!(entries.len(), 191);
    assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let sorted_designs = entries
        .iter()
        .map(|(_, design)| design.clone())
        .collect::<Vec<_>>();
    let sorted_catalog = ModuleCatalog::from_designs(&sorted_designs).unwrap();
    let mut reversed_designs = sorted_designs.clone();
    reversed_designs.reverse();
    let reversed_catalog = ModuleCatalog::from_designs(&reversed_designs).unwrap();
    assert_eq!(
        sorted_catalog, reversed_catalog,
        "catalog contents depend on filesystem traversal order"
    );
    let timing_failures = collect_timing_lower_failures(&entries, &sorted_catalog);
    assert!(
        timing_failures.is_empty(),
        "timing-aware corpus failures:\n{}",
        timing_failures.join("\n")
    );

    let mut all_snapshots = BTreeMap::<(String, String), FileSnapshot>::new();
    let mut summary = String::from("functional timing graph corpus audit\n");
    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        let mut audit = ModeAudit::default();
        let mut forward = BTreeMap::new();
        for (path, design) in &entries {
            let snapshot = audit_file(path, design, &sorted_catalog, mode, &root, &mut audit);
            assert!(
                forward.insert(path.clone(), snapshot.clone()).is_none(),
                "duplicate forward audit path {path}"
            );
            all_snapshots.insert((mode.label().to_string(), path.clone()), snapshot);
        }

        for (path, design) in entries.iter().rev() {
            let mut reversal_audit = ModeAudit::default();
            let reversed = audit_file(
                path,
                design,
                &reversed_catalog,
                mode,
                &root,
                &mut reversal_audit,
            );
            assert_eq!(
                &reversed,
                &forward[path],
                "{path} {} timing model changed under reversed traversal/catalog construction",
                mode.label()
            );
        }

        assert_exact_mode_contract(mode, &audit);
        render_mode_summary(&mut summary, mode, &audit);
    }

    assert!(!summary.contains(&root.to_string_lossy().to_string()));
    assert_clean_report_text(&summary, &root, "corpus summary");
    assert_or_update_fixture("corpus_summary.timing-graph", &summary);

    for (path, fixture) in [
        (TFFNL, "tffnl.delayful.timing-graph"),
        (AO21, "ao21.delayful.timing-graph"),
        (HALF_ADD, "half_add.delayful.timing-graph"),
    ] {
        let snapshot = &all_snapshots[&(GenerateMode::Delayful.label().to_string(), path.into())];
        assert_or_update_fixture(fixture, &snapshot.rendered_report);
    }

    assert_representative_architecture(&all_snapshots);
}

fn audit_file(
    path: &str,
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
    repository_root: &Path,
    audit: &mut ModeAudit,
) -> FileSnapshot {
    audit.files += 1;
    let ordinary = lower_design_with_catalog_and_generate_mode(design, catalog, mode)
        .unwrap_or_else(|diagnostic| panic!("{path} {} ordinary: {diagnostic}", mode.label()));
    let first = lower_design_with_timing_and_catalog_and_generate_mode(design, catalog, mode)
        .unwrap_or_else(|diagnostic| panic!("{path} {} timing: {diagnostic}", mode.label()));
    let second = lower_design_with_timing_and_catalog_and_generate_mode(design, catalog, mode)
        .unwrap_or_else(|diagnostic| {
            panic!("{path} {} repeated timing: {diagnostic}", mode.label())
        });

    first
        .lowered()
        .cell
        .validate()
        .unwrap_or_else(|error| panic!("{path} {} timing cell validation: {error}", mode.label()));
    if first.lowered() != &ordinary {
        audit.ordinary_compatibility_mismatches += 1;
    }
    assert_eq!(
        first.lowered(),
        &ordinary,
        "{path} {} changed the complete ordinary M14 lowering result",
        mode.label()
    );
    assert_timing_models_equal(path, mode, &first, &second);

    let cut = first.cut_graph();
    assert_eq!(cut.node_count(), first.functional_graph().nodes().len());
    assert_eq!(cut.topological_order().len(), cut.node_count());
    assert_eq!(
        cut.topological_order()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len(),
        cut.node_count(),
        "{path} {} cut topological order is incomplete or duplicated",
        mode.label()
    );

    for constraint in first.functional_graph().constraints() {
        if constraint.additive_delay().to_delay_tuple().as_ref() != Ok(constraint.delay()) {
            audit.additive_rebuild_failures += 1;
        }
        assert_eq!(
            constraint.additive_delay().to_delay_tuple().unwrap(),
            *constraint.delay(),
            "{path} {} additive reconstruction mismatch at {}",
            mode.label(),
            constraint.id()
        );
    }

    let rendered = first.timing_analysis().render();
    if report_text_is_unclean(&rendered, repository_root) {
        audit.absolute_path_leaks += 1;
    }
    assert_clean_report_text(&rendered, repository_root, path);
    if rendered != second.timing_analysis().render() {
        audit.nondeterministic_reports += 1;
    }
    assert_eq!(
        rendered,
        second.timing_analysis().render(),
        "{path} {} rendered report changed on repetition",
        mode.label()
    );

    aggregate_report(path, first.timing_analysis(), audit);
    FileSnapshot {
        lowered: first.lowered().clone(),
        assignment_provenance: first.assignment_provenance().to_vec(),
        report: first.timing_analysis().clone(),
        rendered_report: rendered,
    }
}

fn assert_timing_models_equal(
    path: &str,
    mode: GenerateMode,
    first: &sv_to_sexpr::lower::LoweredTimingModel,
    second: &sv_to_sexpr::lower::LoweredTimingModel,
) {
    assert_eq!(first.lowered(), second.lowered(), "{path} {}", mode.label());
    assert_eq!(
        first.assignment_provenance(),
        second.assignment_provenance(),
        "{path} {} assignment provenance",
        mode.label()
    );
    assert_eq!(
        first.functional_graph().nodes().collect::<Vec<_>>(),
        second.functional_graph().nodes().collect::<Vec<_>>(),
        "{path} {} graph nodes",
        mode.label()
    );
    assert_eq!(
        first.functional_graph().dependencies(),
        second.functional_graph().dependencies(),
        "{path} {} graph dependencies",
        mode.label()
    );
    assert_eq!(
        first.functional_graph().constraints(),
        second.functional_graph().constraints(),
        "{path} {} graph constraints",
        mode.label()
    );
    assert_eq!(
        first.cut_graph().nodes(),
        second.cut_graph().nodes(),
        "{path} {} cut nodes",
        mode.label()
    );
    assert_eq!(
        first.cut_graph().dependencies(),
        second.cut_graph().dependencies(),
        "{path} {} cut dependencies",
        mode.label()
    );
    assert_eq!(
        first.cut_graph().excluded_state_boundaries(),
        second.cut_graph().excluded_state_boundaries(),
        "{path} {} state cuts",
        mode.label()
    );
    assert_eq!(
        first.cut_graph().excluded_resolved_net_boundaries(),
        second.cut_graph().excluded_resolved_net_boundaries(),
        "{path} {} resolved-net cuts",
        mode.label()
    );
    assert_eq!(
        first.cut_graph().topological_order(),
        second.cut_graph().topological_order(),
        "{path} {} cut order",
        mode.label()
    );
    assert_eq!(
        first.timing_analysis(),
        second.timing_analysis(),
        "{path} {} typed report",
        mode.label()
    );
}

fn aggregate_report(path: &str, report: &TimingAnalysisReport, audit: &mut ModeAudit) {
    audit.nodes += report.nodes().len();
    for node in report.nodes() {
        match node.kind() {
            TimingNodeKind::Signal(_) => audit.signals += 1,
            TimingNodeKind::Assignment(_) => audit.assignments += 1,
        }
    }
    audit.dependencies += report.dependencies().len();
    for dependency in report.dependencies() {
        increment(
            &mut audit.dependency_kinds,
            dependency_kind_name(dependency.edge().kind()),
        );
        increment(
            &mut audit.dependency_senses,
            timing_sense_name(dependency.edge().sense()),
        );
        match dependency.edge().kind() {
            DependencyKind::StateBoundary => audit.state_boundaries += 1,
            DependencyKind::ResolvedNetBoundary => audit.resolved_net_boundaries += 1,
            DependencyKind::StateControl => increment(
                &mut audit.state_control_transitions,
                transition_name(dependency.edge().event_transition()),
            ),
            DependencyKind::Operand | DependencyKind::Drive => {}
        }
    }

    audit.constraints += report.constraints().len();
    audit.elaborated_constraints += report.constraints().len();
    let root_path = Path::new(path);
    let mut owned_by_target = BTreeMap::<String, Vec<&TimingConstraint>>::new();
    let mut owned_constraints = 0;
    let mut foreign_constraints = 0;
    for constraint in report.constraints() {
        audit.controls += constraint.controls().len();
        audit.elaborated_controls += constraint.controls().len();
        audit.elaborated_tuple_arities[constraint.delay().len()] += 1;
        if constraint.span().path == root_path {
            owned_constraints += 1;
            audit.source_constraints += 1;
            audit.source_controls += constraint.controls().len();
            audit.source_tuple_arities[constraint.delay().len()] += 1;
            assert_eq!(constraint.target_span().path, root_path);
            assert!(
                constraint
                    .controls()
                    .iter()
                    .all(|control| control.source().span().path == root_path)
            );
            owned_by_target
                .entry(constraint.target().to_string())
                .or_default()
                .push(constraint);
        } else {
            foreign_constraints += 1;
            audit.foreign_constraints += 1;
            assert_ne!(constraint.span().path, root_path);
        }
    }
    assert_eq!(
        owned_constraints + foreign_constraints,
        report.constraints().len(),
        "{path} ownership partition omitted or duplicated a constraint"
    );
    if owned_constraints == 0 && foreign_constraints > 0 {
        audit.expanded_only_roots.insert(path.to_string());
    }

    audit.source_target_groups += owned_by_target.len();
    for (target, constraints) in owned_by_target {
        if constraints.len() <= 1 {
            continue;
        }
        audit.source_multiple_target_groups += 1;
        let target_report = report
            .target_groups()
            .iter()
            .find(|group| group.group().target() == target)
            .unwrap_or_else(|| panic!("{path}:{target} missing elaborated target report"));
        let control_count = constraints
            .iter()
            .map(|constraint| constraint.controls().len())
            .sum::<usize>();
        let control_signals = constraints
            .iter()
            .flat_map(|constraint| constraint.controls())
            .map(|control| control.source().signal().to_string())
            .collect::<BTreeSet<_>>();
        let control_summary = control_signals
            .iter()
            .map(|signal| {
                let group = report
                    .control_groups()
                    .iter()
                    .find(|group| group.control_signal() == signal)
                    .unwrap();
                format!(
                    "{}:{}/prefix-nodes={}",
                    signal,
                    control_group_kind_name(group.kind()),
                    group.common_prefix().len()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let key = format!("{path}:{target}");
        let value = format!(
            "paths={} controls={} signals=[{}] target={} suffix-nodes={} reconvergent-nodes={} split={} control-groups=[{}]",
            constraints.len(),
            control_count,
            control_signals.into_iter().collect::<Vec<_>>().join(","),
            target_group_kind_name(target_report.group().kind()),
            target_report.common_suffix().len(),
            target_report.reconvergent_nodes().len(),
            public_split_name(target_report.public_output_split()),
            control_summary
        );
        assert!(
            audit.multiple_witnesses.insert(key, value).is_none(),
            "{path}:{target} duplicate source-owned witness"
        );
    }

    audit.control_groups += report.control_groups().len();
    for group in report.control_groups() {
        increment(
            &mut audit.control_group_kinds,
            control_group_kind_name(group.kind()),
        );
        if !group.common_prefix().is_empty() {
            audit.nonempty_prefix_groups += 1;
            audit.prefix_nodes += group.common_prefix().len();
        }
    }

    audit.target_groups += report.target_groups().len();
    audit.elaborated_target_groups += report.target_groups().len();
    for group in report.target_groups() {
        increment(
            &mut audit.target_group_kinds,
            target_group_kind_name(group.group().kind()),
        );
        increment(
            &mut audit.public_splits,
            public_split_name(group.public_output_split()),
        );
        if !group.common_suffix().is_empty() {
            audit.nonempty_suffix_groups += 1;
            audit.suffix_nodes += group.common_suffix().len();
        }
        if !group.reconvergent_nodes().is_empty() {
            audit.nonempty_reconvergent_groups += 1;
            audit.reconvergent_nodes += group.reconvergent_nodes().len();
        }
        for control in group.control_reports() {
            for sense in control.path_senses() {
                increment(&mut audit.path_senses, path_sense_name(*sense));
            }
        }
    }
}

fn assert_exact_mode_contract(mode: GenerateMode, audit: &ModeAudit) {
    assert_eq!(audit.files, 191, "{}", mode.label());
    assert_eq!(audit.source_constraints, 241, "{}", mode.label());
    assert_eq!(audit.source_controls, 443, "{}", mode.label());
    assert_eq!(audit.source_target_groups, 184, "{}", mode.label());
    assert_eq!(audit.source_multiple_target_groups, 45, "{}", mode.label());
    assert_eq!(audit.source_tuple_arities, [0, 0, 238, 3]);
    assert_eq!(
        audit.source_tuple_arities.iter().sum::<usize>(),
        audit.source_constraints
    );
    assert_eq!(audit.constraints, audit.elaborated_constraints);
    assert_eq!(audit.controls, audit.elaborated_controls);
    assert_eq!(audit.target_groups, audit.elaborated_target_groups);
    assert_eq!(audit.elaborated_constraints, 248);
    assert_eq!(audit.foreign_constraints, 7);
    assert_eq!(audit.elaborated_controls, 457);
    assert_eq!(audit.elaborated_target_groups, 191);
    assert_eq!(audit.elaborated_tuple_arities, [0, 0, 245, 3]);
    assert_eq!(
        (
            audit.nodes,
            audit.signals,
            audit.assignments,
            audit.dependencies
        ),
        (4_295, 2_590, 1_705, 6_251)
    );
    assert_eq!(
        audit.dependency_kinds,
        counts([
            ("drive", 1_555),
            ("operand", 4_546),
            ("resolved-net-boundary", 135),
            ("state-boundary", 15),
        ])
    );
    assert_eq!(
        audit.dependency_senses,
        counts([
            ("conditional", 530),
            ("negative", 317),
            ("non-unate", 82),
            ("positive", 5_322),
        ])
    );
    assert_eq!(audit.state_boundaries, 15);
    assert_eq!(audit.resolved_net_boundaries, 135);
    assert_eq!(audit.state_control_transitions, BTreeMap::new());
    assert_eq!((audit.control_groups, audit.target_groups), (391, 191));
    assert_eq!(
        audit.control_group_kinds,
        counts([("multiple-targets", 43), ("single-target", 348)])
    );
    assert_eq!(
        audit.target_group_kinds,
        counts([("multiple-paths", 45), ("single-path", 146)])
    );
    assert_eq!(
        audit.path_senses,
        counts([
            ("conditional", 59),
            ("negative", 209),
            ("non-unate", 104),
            ("positive", 132),
        ])
    );
    assert_eq!(
        audit.public_splits,
        counts([("candidate", 19), ("not-public", 3), ("not-required", 169)])
    );
    assert_eq!(
        (
            audit.nonempty_prefix_groups,
            audit.prefix_nodes,
            audit.nonempty_suffix_groups,
            audit.suffix_nodes,
            audit.nonempty_reconvergent_groups,
            audit.reconvergent_nodes,
        ),
        (367, 605, 191, 206, 123, 265)
    );
    assert_eq!(audit.multiple_witnesses.len(), 45);
    assert_eq!(audit.combinational_cycle_errors, 0);
    assert_eq!(audit.unreachable_control_errors, 0);
    assert_eq!(audit.additive_rebuild_failures, 0);
    assert_eq!(audit.nondeterministic_reports, 0);
    assert_eq!(audit.ordinary_compatibility_mismatches, 0);
    assert_eq!(audit.absolute_path_leaks, 0);
    assert_eq!(
        audit.expanded_only_roots,
        BTreeSet::from([
            HALF_ADD.to_string(),
            "sv-cells/dmg_cpu_b/cells/full_add.sv".to_string()
        ])
    );
}

fn counts<const N: usize>(entries: [(&str, usize); N]) -> BTreeMap<String, usize> {
    entries
        .into_iter()
        .map(|(name, count)| (name.to_string(), count))
        .collect()
}

fn render_mode_summary(output: &mut String, mode: GenerateMode, audit: &ModeAudit) {
    writeln!(output, "mode={}", mode.label()).unwrap();
    writeln!(
        output,
        "  source-owned files={} constraints={} controls={} target-groups={} target-groups-single={} target-groups-multiple={} tuples=one:{},two:{},three:{}",
        audit.files,
        audit.source_constraints,
        audit.source_controls,
        audit.source_target_groups,
        audit.source_target_groups - audit.source_multiple_target_groups,
        audit.source_multiple_target_groups,
        audit.source_tuple_arities[1],
        audit.source_tuple_arities[2],
        audit.source_tuple_arities[3]
    )
    .unwrap();
    writeln!(
        output,
        "  elaborated constraints={} foreign-constraints={} controls={} target-groups={} tuples=one:{},two:{},three:{} expanded-only-roots=[{}]",
        audit.elaborated_constraints,
        audit.foreign_constraints,
        audit.elaborated_controls,
        audit.elaborated_target_groups,
        audit.elaborated_tuple_arities[1],
        audit.elaborated_tuple_arities[2],
        audit.elaborated_tuple_arities[3],
        audit
            .expanded_only_roots
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    )
    .unwrap();
    writeln!(
        output,
        "  graph nodes={} signals={} assignments={} dependencies={} dependency-kinds={} dependency-senses={}",
        audit.nodes,
        audit.signals,
        audit.assignments,
        audit.dependencies,
        render_counts(&audit.dependency_kinds),
        render_counts(&audit.dependency_senses)
    )
    .unwrap();
    writeln!(
        output,
        "  boundaries state={} resolved-net={} state-controls={} transitions={}",
        audit.state_boundaries,
        audit.resolved_net_boundaries,
        audit
            .dependency_kinds
            .get("state-control")
            .copied()
            .unwrap_or(0),
        render_counts(&audit.state_control_transitions)
    )
    .unwrap();
    writeln!(
        output,
        "  groups constraints={} controls={} control-groups={} control-kinds={} target-groups={} target-kinds={}",
        audit.constraints,
        audit.controls,
        audit.control_groups,
        render_counts(&audit.control_group_kinds),
        audit.target_groups,
        render_counts(&audit.target_group_kinds)
    )
    .unwrap();
    writeln!(
        output,
        "  classifications path-senses={} public-splits={} prefixes=groups:{},nodes:{} suffixes=groups:{},nodes:{} reconvergence=groups:{},nodes:{}",
        render_counts(&audit.path_senses),
        render_counts(&audit.public_splits),
        audit.nonempty_prefix_groups,
        audit.prefix_nodes,
        audit.nonempty_suffix_groups,
        audit.suffix_nodes,
        audit.nonempty_reconvergent_groups,
        audit.reconvergent_nodes
    )
    .unwrap();
    writeln!(
        output,
        "  failures combinational-cycles={} unreachable-controls={} additive-rebuild={} nondeterministic-reports={} ordinary-compatibility={} absolute-path-leaks={}",
        audit.combinational_cycle_errors,
        audit.unreachable_control_errors,
        audit.additive_rebuild_failures,
        audit.nondeterministic_reports,
        audit.ordinary_compatibility_mismatches,
        audit.absolute_path_leaks
    )
    .unwrap();
    writeln!(output, "  source-owned-multiple-path-witnesses:").unwrap();
    for (key, value) in &audit.multiple_witnesses {
        writeln!(output, "    {key} {value}").unwrap();
    }
}

fn assert_representative_architecture(snapshots: &BTreeMap<(String, String), FileSnapshot>) {
    let delayful = GenerateMode::Delayful.label().to_string();
    let tffnl = &snapshots[&(delayful.clone(), TFFNL.to_string())].report;
    assert_eq!(tffnl.constraints().len(), 6);
    assert!(
        tffnl
            .control_groups()
            .iter()
            .filter(|group| matches!(group.control_signal(), "tclk_n" | "d" | "l"))
            .all(|group| {
                group.kind() == ControlGroupKind::MultipleTargets
                    && !group.common_prefix().is_empty()
            })
    );
    for target in ["q", "q_n"] {
        let group = tffnl
            .target_groups()
            .iter()
            .find(|group| group.group().target() == target)
            .unwrap();
        assert!(!group.common_suffix().is_empty());
        assert_eq!(group.group().kind(), TargetGroupKind::MultiplePaths);
    }
    assert_eq!(
        tffnl
            .target_groups()
            .iter()
            .find(|group| group.group().target() == "q")
            .unwrap()
            .public_output_split(),
        PublicOutputSplit::Candidate
    );
    assert_eq!(
        tffnl
            .target_groups()
            .iter()
            .find(|group| group.group().target() == "q_n")
            .unwrap()
            .public_output_split(),
        PublicOutputSplit::NotRequired
    );
    assert!(
        tffnl
            .excluded_state_boundaries()
            .iter()
            .any(|dependency| dependency.edge().kind() == DependencyKind::StateBoundary)
    );

    let ao21 = &snapshots[&(delayful.clone(), AO21.to_string())].report;
    assert_eq!(ao21.constraints().len(), 2);
    assert_eq!(ao21.constraints()[0].controls().len(), 2);
    assert_eq!(ao21.constraints()[1].controls().len(), 1);
    let y = ao21
        .target_groups()
        .iter()
        .find(|group| group.group().target() == "y")
        .unwrap();
    assert_eq!(y.group().kind(), TargetGroupKind::MultiplePaths);
    assert!(!y.common_suffix().is_empty());
    assert!(!y.reconvergent_nodes().is_empty());

    let half_add = &snapshots[&(delayful, HALF_ADD.to_string())].report;
    assert!(!half_add.constraints().is_empty());
    assert!(
        half_add
            .constraints()
            .iter()
            .all(|constraint| constraint.span().path != Path::new(HALF_ADD))
    );
    assert!(
        half_add
            .constraints()
            .windows(2)
            .all(|pair| pair[0].path_order() < pair[1].path_order())
    );
    assert!(half_add.constraints().iter().all(|constraint| {
        constraint.additive_delay().to_delay_tuple().unwrap() == *constraint.delay()
    }));

    let b2b = &snapshots[&(
        GenerateMode::Delayful.label().to_string(),
        B2B_WAND.to_string(),
    )]
        .report;
    assert!(b2b.excluded_state_boundaries().is_empty());
    assert_eq!(b2b.excluded_resolved_net_boundaries().len(), 5);
    let resolved_names = b2b
        .nodes()
        .iter()
        .filter_map(|node| match node.kind() {
            TimingNodeKind::Signal(signal)
                if signal
                    .roles()
                    .contains(&sv_to_sexpr::timing_graph::TimingSignalRole::ResolvedNet) =>
            {
                Some(signal.name())
            }
            TimingNodeKind::Signal(_) | TimingNodeKind::Assignment(_) => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(resolved_names, BTreeSet::from(["a", "b"]));
    assert_eq!(
        b2b.dependencies()
            .iter()
            .filter(|dependency| {
                dependency.edge().kind() == DependencyKind::ResolvedNetBoundary
            })
            .count(),
        5,
        "all resolution exclusions remain present in the full graph"
    );
    assert_eq!(
        b2b.dependencies()
            .iter()
            .filter(|dependency| {
                dependency.edge().kind() == DependencyKind::ResolvedNetBoundary
            })
            .collect::<Vec<_>>(),
        b2b.excluded_resolved_net_boundaries()
            .iter()
            .collect::<Vec<_>>(),
        "resolution cuts preserve full-graph source driver order"
    );
    assert_eq!(
        b2b.excluded_resolved_net_boundaries()
            .iter()
            .map(|dependency| {
                let node = b2b
                    .nodes()
                    .iter()
                    .find(|node| node.id() == dependency.target())
                    .unwrap();
                match node.kind() {
                    TimingNodeKind::Signal(signal) => signal.name(),
                    TimingNodeKind::Assignment(_) => {
                        panic!("resolved-net boundary target is not a signal")
                    }
                }
            })
            .collect::<Vec<_>>(),
        vec!["a", "a", "a", "b", "b"]
    );
}

fn parse_sorted_corpus(root: &Path) -> Vec<(String, Design)> {
    let mut paths = collect_sv_files(&root.join("sv-cells")).unwrap();
    paths.sort();
    paths
        .into_iter()
        .map(|physical| {
            let logical = normalized_relative(physical.strip_prefix(root).unwrap());
            let source = fs::read_to_string(&physical).unwrap();
            let design = parse_file(Path::new(&logical), &source)
                .unwrap_or_else(|diagnostic| panic!("{logical}: {diagnostic}"));
            (logical, design)
        })
        .collect()
}

fn collect_timing_lower_failures(
    entries: &[(String, Design)],
    catalog: &ModuleCatalog,
) -> Vec<String> {
    let mut failures = Vec::new();
    for mode in [GenerateMode::Delayful, GenerateMode::Nodelay] {
        for (path, design) in entries {
            if let Err(diagnostic) =
                lower_design_with_timing_and_catalog_and_generate_mode(design, catalog, mode)
            {
                failures.push(format!(
                    "{} {path} {}:{}:{} {}",
                    mode.label(),
                    diagnostic.span.path.display(),
                    diagnostic.span.line,
                    diagnostic.span.column,
                    diagnostic.message
                ));
            }
        }
    }
    failures
}

fn assert_or_update_fixture(name: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing_graph")
        .join(name);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(actual, expected, "timing graph fixture changed: {name}");
}

fn assert_clean_report_text(actual: &str, repository_root: &Path, context: &str) {
    assert!(
        !report_text_is_unclean(actual, repository_root),
        "{context} contains environment-dependent graph text"
    );
}

fn report_text_is_unclean(actual: &str, repository_root: &Path) -> bool {
    actual.contains(&repository_root.to_string_lossy().to_string())
        || actual.contains("NodeIndex")
        || actual.contains("0x")
        || actual.contains("/target/")
        || actual.contains("\\target\\")
}

fn increment(counts: &mut BTreeMap<String, usize>, key: impl Into<String>) {
    *counts.entry(key.into()).or_default() += 1;
}

fn render_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn dependency_kind_name(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Operand => "operand",
        DependencyKind::Drive => "drive",
        DependencyKind::StateBoundary => "state-boundary",
        DependencyKind::ResolvedNetBoundary => "resolved-net-boundary",
        DependencyKind::StateControl => "state-control",
    }
}

fn timing_sense_name(sense: TimingSense) -> &'static str {
    match sense {
        TimingSense::PositiveUnate => "positive",
        TimingSense::NegativeUnate => "negative",
        TimingSense::NonUnate => "non-unate",
        TimingSense::Conditional => "conditional",
        TimingSense::StateControl => "state-control",
    }
}

fn transition_name(transition: Option<Transition>) -> &'static str {
    match transition {
        Some(Transition::Rise) => "rise",
        Some(Transition::Fall) => "fall",
        Some(Transition::TurnOff) => "turn-off",
        None => "level",
    }
}

fn path_sense_name(sense: TimingPathSense) -> String {
    match sense {
        TimingPathSense::PositiveUnate => "positive".to_string(),
        TimingPathSense::NegativeUnate => "negative".to_string(),
        TimingPathSense::NonUnate => "non-unate".to_string(),
        TimingPathSense::Conditional => "conditional".to_string(),
        TimingPathSense::StateControl {
            event_transition,
            target_effect,
        } => format!(
            "state-{}-{}",
            transition_name(event_transition),
            match target_effect {
                Some(TransitionEffect::Exact(transition)) => transition_name(Some(transition)),
                Some(TransitionEffect::Indeterminate) => "indeterminate",
                None => "level",
            }
        ),
    }
}

fn control_group_kind_name(kind: ControlGroupKind) -> &'static str {
    match kind {
        ControlGroupKind::SingleTarget => "single-target",
        ControlGroupKind::MultipleTargets => "multiple-targets",
    }
}

fn target_group_kind_name(kind: TargetGroupKind) -> &'static str {
    match kind {
        TargetGroupKind::SinglePath => "single-path",
        TargetGroupKind::MultiplePaths => "multiple-paths",
    }
}

fn public_split_name(split: PublicOutputSplit) -> &'static str {
    match split {
        PublicOutputSplit::NotPublic => "not-public",
        PublicOutputSplit::NotRequired => "not-required",
        PublicOutputSplit::Candidate => "candidate",
    }
}

fn normalized_relative(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
