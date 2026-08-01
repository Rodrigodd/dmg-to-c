//! Deterministic Milestone 17 closure inventory for opt-in exact timing lowering.
//!
//! Each nextest-native shard independently parses the complete retained corpus
//! and constructs its complete hierarchy catalog. The shard owns only paths
//! whose global sorted index is congruent to its index modulo `SHARD_COUNT`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::analyze::ModuleCatalog;
use sv_to_sexpr::ast::Design;
use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::CellItem;
use sv_to_sexpr::lower::{
    LoweredDecomposedTimingModel, lower_design_with_decomposed_timing_and_catalog_and_generate_mode,
};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::collect_sv_files;
use sv_to_sexpr::timing_decompose::{DecompositionPathId, VerifiedDelayComponent};
use sv_to_sexpr::timing_graph::AssignmentDelayOrigin;

const CORPUS_SIZE: usize = 191;
const SHARD_COUNT: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuccessRecord {
    path: String,
    assignments: usize,
    registers: usize,
    constraints: usize,
    components: usize,
    timing_identities: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FailureRecord {
    path: String,
    span_path: String,
    line: usize,
    column: usize,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaseRecord {
    Success(SuccessRecord),
    Failure(FailureRecord),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ShardTotals {
    successes: usize,
    failures: usize,
    assignments: usize,
    registers: usize,
    constraints: usize,
    components: usize,
    timing_identities: usize,
}

macro_rules! corpus_shard_tests {
    ($(($name:ident, $mode:ident, $index:expr, $remainder:expr, $modulus:expr, $suffix:expr)),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                audit_shard(GenerateMode::$mode, $index, $remainder, $modulus, $suffix);
            }
        )+
    };
}

corpus_shard_tests!(
    (corpus_delayful_00, Delayful, 0, 0, 1, ""),
    (corpus_delayful_01a, Delayful, 1, 0, 2, "a"),
    (corpus_delayful_01b0_01, Delayful, 1, 1, 12, "b0-01"),
    (corpus_delayful_01b0_05, Delayful, 1, 5, 12, "b0-05"),
    (corpus_delayful_01b0_09, Delayful, 1, 9, 12, "b0-09"),
    (corpus_delayful_01b1, Delayful, 1, 3, 4, "b1"),
    (corpus_delayful_02, Delayful, 2, 0, 1, ""),
    (corpus_delayful_03, Delayful, 3, 0, 1, ""),
    (corpus_delayful_04, Delayful, 4, 0, 1, ""),
    (corpus_delayful_05, Delayful, 5, 0, 1, ""),
    (corpus_delayful_06, Delayful, 6, 0, 1, ""),
    (corpus_delayful_07a, Delayful, 7, 0, 2, "a"),
    (corpus_delayful_07b, Delayful, 7, 1, 2, "b"),
    (corpus_delayful_08, Delayful, 8, 0, 1, ""),
    (corpus_delayful_09a0_00, Delayful, 9, 0, 12, "a0-00"),
    (corpus_delayful_09a0_04, Delayful, 9, 4, 12, "a0-04"),
    (corpus_delayful_09a0_08, Delayful, 9, 8, 12, "a0-08"),
    (corpus_delayful_09a1, Delayful, 9, 2, 4, "a1"),
    (corpus_delayful_09b, Delayful, 9, 1, 2, "b"),
    (corpus_delayful_10, Delayful, 10, 0, 1, ""),
    (corpus_delayful_11, Delayful, 11, 0, 1, ""),
    (corpus_delayful_12a0_00, Delayful, 12, 0, 12, "a0-00"),
    (corpus_delayful_12a0_04, Delayful, 12, 4, 12, "a0-04"),
    (corpus_delayful_12a0_08, Delayful, 12, 8, 12, "a0-08"),
    (corpus_delayful_12a1, Delayful, 12, 2, 4, "a1"),
    (corpus_delayful_12b, Delayful, 12, 1, 2, "b"),
    (corpus_delayful_13a, Delayful, 13, 0, 2, "a"),
    (corpus_delayful_13b0, Delayful, 13, 1, 4, "b0"),
    (corpus_delayful_13b1_03, Delayful, 13, 3, 12, "b1-03"),
    (corpus_delayful_13b1_07, Delayful, 13, 7, 12, "b1-07"),
    (corpus_delayful_13b1_11, Delayful, 13, 11, 12, "b1-11"),
    (corpus_delayful_14, Delayful, 14, 0, 1, ""),
    (corpus_delayful_15, Delayful, 15, 0, 1, ""),
    (corpus_nodelay_00, Nodelay, 0, 0, 1, ""),
    (corpus_nodelay_01a, Nodelay, 1, 0, 2, "a"),
    (corpus_nodelay_01b0_01, Nodelay, 1, 1, 12, "b0-01"),
    (corpus_nodelay_01b0_05, Nodelay, 1, 5, 12, "b0-05"),
    (corpus_nodelay_01b0_09, Nodelay, 1, 9, 12, "b0-09"),
    (corpus_nodelay_01b1, Nodelay, 1, 3, 4, "b1"),
    (corpus_nodelay_02, Nodelay, 2, 0, 1, ""),
    (corpus_nodelay_03, Nodelay, 3, 0, 1, ""),
    (corpus_nodelay_04, Nodelay, 4, 0, 1, ""),
    (corpus_nodelay_05, Nodelay, 5, 0, 1, ""),
    (corpus_nodelay_06, Nodelay, 6, 0, 1, ""),
    (corpus_nodelay_07a, Nodelay, 7, 0, 2, "a"),
    (corpus_nodelay_07b, Nodelay, 7, 1, 2, "b"),
    (corpus_nodelay_08, Nodelay, 8, 0, 1, ""),
    (corpus_nodelay_09a0_00, Nodelay, 9, 0, 12, "a0-00"),
    (corpus_nodelay_09a0_04, Nodelay, 9, 4, 12, "a0-04"),
    (corpus_nodelay_09a0_08, Nodelay, 9, 8, 12, "a0-08"),
    (corpus_nodelay_09a1, Nodelay, 9, 2, 4, "a1"),
    (corpus_nodelay_09b, Nodelay, 9, 1, 2, "b"),
    (corpus_nodelay_10, Nodelay, 10, 0, 1, ""),
    (corpus_nodelay_11, Nodelay, 11, 0, 1, ""),
    (corpus_nodelay_12a0_00, Nodelay, 12, 0, 12, "a0-00"),
    (corpus_nodelay_12a0_04, Nodelay, 12, 4, 12, "a0-04"),
    (corpus_nodelay_12a0_08, Nodelay, 12, 8, 12, "a0-08"),
    (corpus_nodelay_12a1, Nodelay, 12, 2, 4, "a1"),
    (corpus_nodelay_12b, Nodelay, 12, 1, 2, "b"),
    (corpus_nodelay_13a, Nodelay, 13, 0, 2, "a"),
    (corpus_nodelay_13b0, Nodelay, 13, 1, 4, "b0"),
    (corpus_nodelay_13b1_03, Nodelay, 13, 3, 12, "b1-03"),
    (corpus_nodelay_13b1_07, Nodelay, 13, 7, 12, "b1-07"),
    (corpus_nodelay_13b1_11, Nodelay, 13, 11, 12, "b1-11"),
    (corpus_nodelay_14, Nodelay, 14, 0, 1, ""),
    (corpus_nodelay_15, Nodelay, 15, 0, 1, ""),
);

#[test]
fn corpus_partition_manifest_is_complete() {
    let root = repository_root();
    let paths = sorted_corpus_paths(&root);
    assert_eq!(paths.len(), CORPUS_SIZE, "curated corpus size changed");
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let unique_paths = paths.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(unique_paths.len(), CORPUS_SIZE, "duplicate logical path");
    for path in &paths {
        let basename = Path::new(path)
            .file_name()
            .expect("corpus path has a basename")
            .to_string_lossy();
        assert!(
            !basename.starts_with("dff"),
            "retired DFF family remains in retained corpus: {path}"
        );
    }

    let mut assignments = BTreeSet::new();
    let mut per_shard = [[0_usize; SHARD_COUNT]; 2];
    let mut per_physical_shard = BTreeMap::new();
    for (mode_index, mode) in [GenerateMode::Delayful, GenerateMode::Nodelay]
        .into_iter()
        .enumerate()
    {
        for (global_index, path) in paths.iter().enumerate() {
            let shard = global_index % SHARD_COUNT;
            let rank = global_index / SHARD_COUNT;
            assert!(
                assignments.insert((mode.label(), path.as_str())),
                "duplicate shard assignment for {} {path}",
                mode.label()
            );
            per_shard[mode_index][shard] += 1;
            *per_physical_shard
                .entry((mode.label(), shard, physical_shard(shard, rank)))
                .or_insert(0_usize) += 1;
        }
    }

    assert_eq!(assignments.len(), CORPUS_SIZE * 2);
    for counts in per_shard {
        assert!(counts.into_iter().all(|count| count > 0));
        assert_eq!(counts.into_iter().sum::<usize>(), CORPUS_SIZE);
    }
    assert_eq!(per_physical_shard.len(), 66);
    assert!(per_physical_shard.values().all(|count| *count > 0));

    let expected = [GenerateMode::Delayful, GenerateMode::Nodelay]
        .into_iter()
        .flat_map(|mode| paths.iter().map(move |path| (mode.label(), path.as_str())))
        .collect::<BTreeSet<_>>();
    assert_eq!(assignments, expected, "shard union has gaps or duplicates");
}

fn audit_shard(
    mode: GenerateMode,
    shard: usize,
    remainder: usize,
    modulus: usize,
    suffix: &'static str,
) {
    assert!(shard < SHARD_COUNT);
    assert!(remainder < modulus);
    let root = repository_root();
    let entries = parse_sorted_corpus(&root);
    assert_eq!(entries.len(), CORPUS_SIZE, "curated corpus size changed");
    assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let designs = entries
        .iter()
        .map(|(_, design)| design.clone())
        .collect::<Vec<_>>();
    let catalog = ModuleCatalog::from_designs(&designs).expect("complete corpus catalog");
    let assigned = entries
        .iter()
        .enumerate()
        .filter(|(global_index, _)| {
            let rank = global_index / SHARD_COUNT;
            let selected = global_index % SHARD_COUNT == shard && rank % modulus == remainder;
            if selected {
                assert_eq!(physical_shard(shard, rank), suffix);
            }
            selected
        })
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    assert!(!assigned.is_empty(), "empty corpus shard {shard}");

    let records = assigned
        .into_iter()
        .map(|(path, design)| audit_case(path, design, &catalog, mode))
        .collect::<Vec<_>>();
    let fixture = render_shard(mode, shard, remainder, modulus, suffix, &records);
    assert!(!fixture.contains(&root.to_string_lossy().to_string()));
    assert_or_update_fixture(mode, shard, suffix, &fixture);
}

fn physical_shard(shard: usize, rank: usize) -> &'static str {
    match (shard, rank) {
        (1, 0 | 2 | 4 | 6 | 8 | 10)
        | (7, 0 | 2 | 4 | 6 | 8 | 10)
        | (13, 0 | 2 | 4 | 6 | 8 | 10) => "a",
        (1, 1) => "b0-01",
        (1, 5) => "b0-05",
        (1, 9) => "b0-09",
        (1, 3 | 7 | 11) => "b1",
        (7, 1 | 3 | 5 | 7 | 9 | 11)
        | (9, 1 | 3 | 5 | 7 | 9 | 11)
        | (12, 1 | 3 | 5 | 7 | 9 | 11) => "b",
        (9, 0) | (12, 0) => "a0-00",
        (9, 4) | (12, 4) => "a0-04",
        (9, 8) | (12, 8) => "a0-08",
        (9, 2 | 6 | 10) | (12, 2 | 6 | 10) => "a1",
        (13, 1 | 5 | 9) => "b0",
        (13, 3) => "b1-03",
        (13, 7) => "b1-07",
        (13, 11) => "b1-11",
        _ => "",
    }
}

fn audit_case(
    path: &str,
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> CaseRecord {
    let first =
        lower_design_with_decomposed_timing_and_catalog_and_generate_mode(design, catalog, mode);
    let second =
        lower_design_with_decomposed_timing_and_catalog_and_generate_mode(design, catalog, mode);

    match (first, second) {
        (Ok(first), Ok(second)) => validate_success(path, mode, &first, &second),
        (Err(first), Err(second)) => {
            assert_eq!(
                first,
                second,
                "{path} {} diagnostic changed on repetition",
                mode.label()
            );
            CaseRecord::Failure(failure_record(path, first))
        }
        (Ok(_), Err(diagnostic)) | (Err(diagnostic), Ok(_)) => panic!(
            "{path} {} lowering success/failure changed on repetition: {diagnostic}",
            mode.label()
        ),
    }
}

fn validate_success(
    path: &str,
    mode: GenerateMode,
    first: &LoweredDecomposedTimingModel,
    second: &LoweredDecomposedTimingModel,
) -> CaseRecord {
    validate_model(path, mode, first);
    validate_model(path, mode, second);

    assert_eq!(first.lowered(), second.lowered(), "{path} lowered model");
    assert_eq!(
        first.decomposition(),
        second.decomposition(),
        "{path} decomposition"
    );
    assert_eq!(
        first.applied_facts(),
        second.applied_facts(),
        "{path} applied timing facts"
    );
    assert_eq!(
        first.actual_verification(),
        second.actual_verification(),
        "{path} actual verification"
    );
    assert_eq!(
        first.assignment_provenance(),
        second.assignment_provenance(),
        "{path} assignment provenance"
    );
    assert_eq!(
        first.signal_metadata(),
        second.signal_metadata(),
        "{path} signal metadata"
    );
    assert!(
        first
            .functional_graph()
            .nodes()
            .eq(second.functional_graph().nodes()),
        "{path} functional graph nodes"
    );
    assert_eq!(
        first.functional_graph().dependencies(),
        second.functional_graph().dependencies(),
        "{path} functional graph dependencies"
    );
    assert_eq!(
        first.functional_graph().constraints(),
        second.functional_graph().constraints(),
        "{path} functional graph constraints"
    );
    assert_eq!(
        first.cut_graph().nodes(),
        second.cut_graph().nodes(),
        "{path} cut graph nodes"
    );
    assert_eq!(
        first.cut_graph().dependencies(),
        second.cut_graph().dependencies(),
        "{path} cut graph dependencies"
    );
    assert_eq!(
        first.cut_graph().excluded_state_boundaries(),
        second.cut_graph().excluded_state_boundaries(),
        "{path} cut graph state boundaries"
    );
    assert_eq!(
        first.cut_graph().excluded_resolved_net_boundaries(),
        second.cut_graph().excluded_resolved_net_boundaries(),
        "{path} cut graph resolved-net boundaries"
    );
    assert_eq!(
        first.cut_graph().topological_order(),
        second.cut_graph().topological_order(),
        "{path} cut graph topological order"
    );
    assert_eq!(
        first.timing_analysis(),
        second.timing_analysis(),
        "{path} timing analysis"
    );

    let first_erased = first
        .erasure()
        .erase(first.lowered(), first.assignment_provenance())
        .unwrap_or_else(|error| panic!("{path} {} erasure: {error}", mode.label()));
    let second_erased = second
        .erasure()
        .erase(second.lowered(), second.assignment_provenance())
        .unwrap_or_else(|error| panic!("{path} {} repeated erasure: {error}", mode.label()));
    assert_eq!(
        first_erased.lowered(),
        second_erased.lowered(),
        "{path} erased baseline lowered model"
    );
    assert_eq!(
        first_erased.assignment_provenance(),
        second_erased.assignment_provenance(),
        "{path} erased baseline provenance"
    );
    assert_eq!(
        first_erased.signal_metadata(),
        second_erased.signal_metadata(),
        "{path} erased baseline metadata"
    );
    assert!(
        first_erased.lowered().diagnostics.is_empty(),
        "{path} {} erased baseline retained diagnostics",
        mode.label()
    );

    let expected_components = expected_constraint_components(first);
    CaseRecord::Success(SuccessRecord {
        path: path.to_string(),
        assignments: first.assignment_provenance().len(),
        registers: first.lowered().cell.registers.len(),
        constraints: first.functional_graph().constraints().len(),
        components: expected_components.values().map(Vec::len).sum(),
        timing_identities: first
            .assignment_provenance()
            .iter()
            .filter(|value| value.origin().is_timing_identity())
            .count(),
    })
}

fn validate_model(path: &str, mode: GenerateMode, model: &LoweredDecomposedTimingModel) {
    assert!(
        model.lowered().diagnostics.is_empty(),
        "{path} {} retained diagnostics: {:?}",
        mode.label(),
        model.lowered().diagnostics
    );
    model
        .lowered()
        .cell
        .validate()
        .unwrap_or_else(|error| panic!("{path} {} invalid cell: {error}", mode.label()));
    assert!(
        model
            .lowered()
            .cell
            .items
            .iter()
            .all(|item| matches!(item, CellItem::Assignment(_))),
        "{path} {} contains non-assignment item",
        mode.label()
    );
    assert!(
        model.assignment_provenance().iter().all(|provenance| {
            provenance.delay_origin() != AssignmentDelayOrigin::LegacySelectedSpecifyFallback
        }),
        "{path} {} retained a legacy selected-first delay",
        mode.label()
    );

    let rendered = render_cell(&model.lowered().cell);
    let canonical = sexpr_fmt::format_source_default(&rendered).unwrap_or_else(|error| {
        panic!("{path} {} formatter rejected output: {error}", mode.label())
    });
    assert_eq!(
        rendered,
        canonical,
        "{path} {} output is not canonical",
        mode.label()
    );

    let expected = expected_constraint_components(model);
    let symbolic = verified_path_component_map(
        path,
        mode,
        model
            .decomposition()
            .verification()
            .paths()
            .iter()
            .map(|verified| {
                (
                    verified.path_id(),
                    verified.constraint_id().ordinal(),
                    verified.control_id().ordinal(),
                    verified.components().to_vec(),
                )
            }),
    );
    let actual = verified_path_component_map(
        path,
        mode,
        model
            .actual_verification()
            .symbolic()
            .paths()
            .iter()
            .map(|verified| {
                (
                    verified.path_id(),
                    verified.constraint_id().ordinal(),
                    verified.control_id().ordinal(),
                    verified.components().to_vec(),
                )
            }),
    );
    assert_eq!(
        symbolic,
        actual,
        "{path} {} symbolic/actual path tuple cover",
        mode.label()
    );
    let mut covered_constraint_controls = BTreeSet::new();
    for ((_, constraint, control), components) in &symbolic {
        assert_eq!(
            expected.get(&(*constraint, *control)),
            Some(components),
            "{path} {} full tuple for c{constraint}/k{control}",
            mode.label()
        );
        covered_constraint_controls.insert((*constraint, *control));
    }
    assert_eq!(
        covered_constraint_controls,
        expected.keys().copied().collect(),
        "{path} {} retained constraint/control tuple cover",
        mode.label()
    );

    let expected_paths = model
        .decomposition()
        .paths()
        .iter()
        .map(|decomposed| {
            (
                decomposed.id(),
                decomposed.constraint_id().ordinal(),
                decomposed.control_id().ordinal(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        symbolic.keys().copied().collect::<BTreeSet<_>>(),
        expected_paths,
        "{path} {} symbolic full-path cover",
        mode.label()
    );
    let applied_paths = model
        .actual_verification()
        .applied()
        .paths()
        .iter()
        .map(|applied| {
            assert!(
                !applied.assignment_orders().is_empty(),
                "{path} {} applied path has no assignments",
                mode.label()
            );
            (
                applied.path_id(),
                applied.constraint_id().ordinal(),
                applied.control_id().ordinal(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        applied_paths,
        expected_paths,
        "{path} {} applied full-tuple path cover",
        mode.label()
    );
    assert_eq!(
        model.actual_verification().checked_placements(),
        model.applied_facts().placements(),
        "{path} {} checked applied placements",
        mode.label()
    );
    assert_eq!(
        model.actual_verification().checked_placements().len(),
        model.decomposition().placements().len(),
        "{path} {} applied placement count",
        mode.label()
    );
}

fn expected_constraint_components(
    model: &LoweredDecomposedTimingModel,
) -> BTreeMap<(u32, u32), Vec<VerifiedDelayComponent>> {
    model
        .functional_graph()
        .constraints()
        .iter()
        .flat_map(|constraint| {
            let components = match constraint.delay().len() {
                1 => vec![VerifiedDelayComponent::All],
                2 => vec![VerifiedDelayComponent::Rise, VerifiedDelayComponent::Fall],
                3 => vec![
                    VerifiedDelayComponent::Rise,
                    VerifiedDelayComponent::Fall,
                    VerifiedDelayComponent::TurnOff,
                ],
                arity => panic!("invalid delay tuple arity {arity}"),
            };
            constraint.controls().iter().map(move |control| {
                (
                    (constraint.id().ordinal(), control.id().ordinal()),
                    components.clone(),
                )
            })
        })
        .collect()
}

fn verified_path_component_map(
    path: &str,
    mode: GenerateMode,
    values: impl Iterator<Item = (DecompositionPathId, u32, u32, Vec<VerifiedDelayComponent>)>,
) -> BTreeMap<(DecompositionPathId, u32, u32), Vec<VerifiedDelayComponent>> {
    let mut result = BTreeMap::new();
    for (path_id, constraint, control, components) in values {
        assert!(
            result
                .insert((path_id, constraint, control), components)
                .is_none(),
            "{path} {} duplicate verification for {path_id}/c{constraint}/k{control}",
            mode.label()
        );
    }
    result
}

fn failure_record(path: &str, diagnostic: Diagnostic) -> FailureRecord {
    FailureRecord {
        path: path.to_string(),
        span_path: normalized_relative(&diagnostic.span.path),
        line: diagnostic.span.line,
        column: diagnostic.span.column,
        message: diagnostic.message,
    }
}

fn render_shard(
    mode: GenerateMode,
    shard: usize,
    remainder: usize,
    modulus: usize,
    suffix: &str,
    records: &[CaseRecord],
) -> String {
    let mut output = String::new();
    writeln!(output, "timing decomposition corpus shard").unwrap();
    writeln!(output, "mode: {}", mode.label()).unwrap();
    writeln!(output, "shard: {shard:02}{suffix}/{SHARD_COUNT}").unwrap();
    writeln!(output, "rule: sorted-index-modulo-{SHARD_COUNT}").unwrap();
    writeln!(output, "split-residue: {remainder}/{modulus}").unwrap();
    writeln!(output, "split-rule: within-shard-rank-modulo-{modulus}").unwrap();
    writeln!(output, "cases: {}", records.len()).unwrap();

    let mut totals = ShardTotals::default();
    for record in records {
        match record {
            CaseRecord::Success(success) => {
                totals.successes += 1;
                totals.assignments += success.assignments;
                totals.registers += success.registers;
                totals.constraints += success.constraints;
                totals.components += success.components;
                totals.timing_identities += success.timing_identities;
                writeln!(
                    output,
                    "success path={} assignments={} registers={} constraints={} components={} dN={}",
                    success.path,
                    success.assignments,
                    success.registers,
                    success.constraints,
                    success.components,
                    success.timing_identities
                )
                .unwrap();
            }
            CaseRecord::Failure(failure) => {
                totals.failures += 1;
                writeln!(
                    output,
                    "failure unclassified path={} span={}:{}:{} message={:?}",
                    failure.path, failure.span_path, failure.line, failure.column, failure.message
                )
                .unwrap();
            }
        }
    }
    writeln!(output, "successes: {}", totals.successes).unwrap();
    writeln!(output, "failures: {}", totals.failures).unwrap();
    writeln!(output, "assignments: {}", totals.assignments).unwrap();
    writeln!(output, "registers: {}", totals.registers).unwrap();
    writeln!(output, "constraints: {}", totals.constraints).unwrap();
    writeln!(output, "components: {}", totals.components).unwrap();
    writeln!(output, "dN: {}", totals.timing_identities).unwrap();
    output
}

fn parse_sorted_corpus(root: &Path) -> Vec<(String, Design)> {
    let mut entries = collect_sv_files(&root.join("sv-cells"))
        .expect("collect retained SystemVerilog corpus")
        .into_iter()
        .map(|physical| {
            let logical = normalized_relative(
                physical
                    .strip_prefix(root)
                    .expect("corpus path is below repository root"),
            );
            let source = fs::read_to_string(&physical)
                .unwrap_or_else(|error| panic!("failed to read {logical}: {error}"));
            let design = parse_file(Path::new(&logical), &source)
                .unwrap_or_else(|diagnostic| panic!("{logical}: {diagnostic}"));
            (logical, design)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn sorted_corpus_paths(root: &Path) -> Vec<String> {
    let mut paths = collect_sv_files(&root.join("sv-cells"))
        .expect("collect retained SystemVerilog corpus")
        .into_iter()
        .map(|physical| {
            normalized_relative(
                physical
                    .strip_prefix(root)
                    .expect("corpus path is below repository root"),
            )
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn assert_or_update_fixture(mode: GenerateMode, shard: usize, suffix: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "tests/fixtures/timing_decomposition/corpus-{}-{shard:02}{suffix}.timing-decomposition",
        mode.label()
    ));
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        fs::write(&fixture, actual)
            .unwrap_or_else(|error| panic!("failed to update {}: {error}", fixture.display()));
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual,
        expected,
        "timing decomposition corpus shard changed: {}",
        fixture.display()
    );
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
        .expect("converter crate is below repository root")
        .to_path_buf()
}
