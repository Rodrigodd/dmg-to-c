//! Typed materialization and exact erasure of a timing decomposition.
//!
//! This layer is deliberately opt-in. It rewrites a zero-specify-delay
//! lowering baseline, records every changed assignment by durable typed ID,
//! and refuses to erase a model whose transformed representation no longer
//! matches the recorded result.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Span;
use crate::ir::{Assignment, CellItem, DelayTuple, Expr, LoweredModule, TimingExpr};
use crate::timing_decompose::{
    AppliedModelSnapshot, AppliedModelVerification, AppliedVerificationMap, Decomposition,
    DecompositionError, DecompositionErrorKind, DecompositionVerification, DelayPlacement,
    PlacementDelay, PlacementSite, verify_applied_model,
};
use crate::timing_graph::{
    AssignmentDelayOrigin, AssignmentOrigin, AssignmentProvenance, DependencyKind,
    SourceAssignmentOrigin, StateControlProvenance, TimingConstraintSource, TimingGraph,
    TimingNodeId, TimingNodeKind, TimingSignalMetadata, TimingSignalRole, analyze_timing_graph,
    build_timing_graph, cut_register_cycles,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppliedAssignmentId {
    Original(usize),
    TimingGenerated(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedPlacement {
    site: PlacementSite,
    assignment_id: AppliedAssignmentId,
    assignment_order: usize,
    delay: DelayTuple,
    empty_components: Vec<bool>,
}

impl AppliedPlacement {
    pub fn site(&self) -> &PlacementSite {
        &self.site
    }

    pub const fn assignment_id(&self) -> AppliedAssignmentId {
        self.assignment_id
    }

    pub const fn assignment_order(&self) -> usize {
        self.assignment_order
    }

    pub fn delay(&self) -> &DelayTuple {
        &self.delay
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedRewrite {
    ExistingAssignment {
        assignment: AppliedAssignmentId,
        before: Assignment,
        after: Assignment,
    },
    OperandEdgeIdentity {
        dependency_order: usize,
        consumer: AppliedAssignmentId,
        identity: AppliedAssignmentId,
        before_consumer: Assignment,
        after_consumer: Assignment,
        inserted: Assignment,
    },
    BoundaryEdgeIdentity {
        dependency_order: usize,
        driver: AppliedAssignmentId,
        identity: AppliedAssignmentId,
        before_driver: Assignment,
        after_driver: Assignment,
        inserted: Assignment,
    },
    PublicOutputSplit {
        signal: String,
        driver: AppliedAssignmentId,
        identity: AppliedAssignmentId,
        before_driver: Assignment,
        after_driver: Assignment,
        inserted: Assignment,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppliedTimingFacts {
    placements: Vec<AppliedPlacement>,
    rewrites: Vec<AppliedRewrite>,
    assignment_orders: BTreeMap<AppliedAssignmentId, usize>,
    original_assignment_orders: BTreeMap<usize, usize>,
}

impl AppliedTimingFacts {
    pub fn placements(&self) -> &[AppliedPlacement] {
        &self.placements
    }

    pub fn rewrites(&self) -> &[AppliedRewrite] {
        &self.rewrites
    }

    /// Total typed identity map for every assignment in the transformed cell.
    pub fn assignment_orders(&self) -> &BTreeMap<AppliedAssignmentId, usize> {
        &self.assignment_orders
    }

    /// Maps each baseline assignment order to its transformed assignment
    /// order, independent of generated identities and source-driver grouping.
    pub fn original_assignment_orders(&self) -> &BTreeMap<usize, usize> {
        &self.original_assignment_orders
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualDecompositionVerification {
    symbolic: DecompositionVerification,
    applied: AppliedModelVerification,
    checked_placements: Vec<AppliedPlacement>,
}

impl ActualDecompositionVerification {
    pub fn symbolic(&self) -> &DecompositionVerification {
        &self.symbolic
    }

    pub fn applied(&self) -> &AppliedModelVerification {
        &self.applied
    }

    pub fn checked_placements(&self) -> &[AppliedPlacement] {
        &self.checked_placements
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasedTimingModel {
    lowered: LoweredModule,
    assignment_provenance: Vec<AssignmentProvenance>,
    signal_metadata: Vec<TimingSignalMetadata>,
}

impl ErasedTimingModel {
    pub fn lowered(&self) -> &LoweredModule {
        &self.lowered
    }

    pub fn assignment_provenance(&self) -> &[AssignmentProvenance] {
        &self.assignment_provenance
    }

    pub fn signal_metadata(&self) -> &[TimingSignalMetadata] {
        &self.signal_metadata
    }

    pub fn into_lowered(self) -> LoweredModule {
        self.lowered
    }
}

/// Exact inverse contract for one applied transform.
///
/// Erasure is intentionally record-driven: it validates the complete expected
/// transformed state before returning the exact original state. It never
/// recognizes generated assignments by a `dN` spelling heuristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingErasure {
    original: ErasedTimingModel,
    expected_lowered: LoweredModule,
    expected_provenance: Vec<AssignmentProvenance>,
    records: Vec<AppliedRewrite>,
    span: Span,
}

impl TimingErasure {
    pub fn records(&self) -> &[AppliedRewrite] {
        &self.records
    }

    pub fn erase(
        &self,
        lowered: &LoweredModule,
        assignment_provenance: &[AssignmentProvenance],
    ) -> Result<ErasedTimingModel, DecompositionError> {
        if lowered != &self.expected_lowered {
            return Err(DecompositionError::new(
                self.span.clone(),
                DecompositionErrorKind::ErasureMismatch {
                    detail: "lowered assignments or interface differ from the typed transform"
                        .to_string(),
                },
            ));
        }
        if assignment_provenance != self.expected_provenance {
            return Err(DecompositionError::new(
                self.span.clone(),
                DecompositionErrorKind::ErasureMismatch {
                    detail: "assignment provenance differs from the typed transform".to_string(),
                },
            ));
        }
        Ok(self.original.clone())
    }
}

#[derive(Debug, Clone)]
pub struct AppliedTimingTransform {
    lowered: LoweredModule,
    assignment_provenance: Vec<AssignmentProvenance>,
    signal_metadata: Vec<TimingSignalMetadata>,
    facts: AppliedTimingFacts,
    verification: ActualDecompositionVerification,
    erasure: TimingErasure,
}

impl AppliedTimingTransform {
    pub fn lowered(&self) -> &LoweredModule {
        &self.lowered
    }

    pub fn assignment_provenance(&self) -> &[AssignmentProvenance] {
        &self.assignment_provenance
    }

    pub fn signal_metadata(&self) -> &[TimingSignalMetadata] {
        &self.signal_metadata
    }

    pub fn facts(&self) -> &AppliedTimingFacts {
        &self.facts
    }

    pub fn verification(&self) -> &ActualDecompositionVerification {
        &self.verification
    }

    pub fn erasure(&self) -> &TimingErasure {
        &self.erasure
    }

    pub fn into_parts(
        self,
    ) -> (
        LoweredModule,
        Vec<AssignmentProvenance>,
        Vec<TimingSignalMetadata>,
        AppliedTimingFacts,
        ActualDecompositionVerification,
        TimingErasure,
    ) {
        (
            self.lowered,
            self.assignment_provenance,
            self.signal_metadata,
            self.facts,
            self.verification,
            self.erasure,
        )
    }
}

#[derive(Debug, Clone)]
struct WorkingProvenance {
    source_assignment_order: usize,
    span: Span,
    origin: AssignmentOrigin,
    delay_origin: AssignmentDelayOrigin,
    state_controls: Vec<StateControlProvenance>,
}

impl WorkingProvenance {
    fn from_public(value: &AssignmentProvenance) -> Self {
        Self {
            source_assignment_order: value.source_assignment_order(),
            span: value.span().clone(),
            origin: value.origin(),
            delay_origin: value.delay_origin(),
            state_controls: value.state_controls().to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
struct WorkingAssignment {
    id: AppliedAssignmentId,
    assignment: Assignment,
    provenance: WorkingProvenance,
}

#[derive(Debug, Clone)]
enum WorkingItem {
    Other(CellItem),
    Assignment(WorkingAssignment),
}

/// Mutable, typed application state used by the two primitive rewrite APIs.
pub struct TimingApplicationState<'a> {
    graph: &'a TimingGraph,
    decomposition: &'a Decomposition,
    original_lowered: LoweredModule,
    original_provenance: Vec<AssignmentProvenance>,
    original_metadata: Vec<TimingSignalMetadata>,
    lowered_shell: LoweredModule,
    items: Vec<WorkingItem>,
    signal_metadata: Vec<TimingSignalMetadata>,
    planned_delay_names: BTreeMap<PlacementSite, String>,
    raw_public_names: BTreeMap<String, String>,
    next_generated_id: usize,
    facts: AppliedTimingFacts,
}

impl<'a> TimingApplicationState<'a> {
    pub fn new(
        lowered: &LoweredModule,
        signal_metadata: &[TimingSignalMetadata],
        assignment_provenance: &[AssignmentProvenance],
        graph: &'a TimingGraph,
        decomposition: &'a Decomposition,
    ) -> Result<Self, DecompositionError> {
        let assignments = lowered
            .cell
            .items
            .iter()
            .filter(|item| matches!(item, CellItem::Assignment(_)))
            .count();
        if assignments != assignment_provenance.len() {
            return Err(application_error(
                first_span(signal_metadata, assignment_provenance),
                DecompositionErrorKind::PlacementConflict {
                    site: decomposition
                        .placements()
                        .first()
                        .map(|placement| placement.site().clone())
                        .unwrap_or(PlacementSite::PublicOutputSplit {
                            signal: graph.nodes().next().map(|node| node.id()).unwrap_or_else(
                                || panic!("empty graph cannot carry a decomposition"),
                            ),
                        }),
                    detail: "assignment provenance is not aligned with the lowered cell"
                        .to_string(),
                },
            ));
        }

        let mut assignment_order = 0;
        let items = lowered
            .cell
            .items
            .iter()
            .cloned()
            .map(|item| match item {
                CellItem::Assignment(assignment) => {
                    let provenance =
                        WorkingProvenance::from_public(&assignment_provenance[assignment_order]);
                    let id = AppliedAssignmentId::Original(assignment_order);
                    assignment_order += 1;
                    WorkingItem::Assignment(WorkingAssignment {
                        id,
                        assignment,
                        provenance,
                    })
                }
                other => WorkingItem::Other(other),
            })
            .collect();

        let mut reserved = BTreeMap::new();
        reserve_lowered_names(
            lowered,
            graph,
            signal_metadata,
            assignment_provenance,
            &mut reserved,
        );
        let mut planned_delay_names = BTreeMap::new();
        let mut next_delay_index = 0;
        for placement in decomposition.placements() {
            if matches!(
                placement.site(),
                PlacementSite::DependencyEdge { .. } | PlacementSite::PublicOutputSplit { .. }
            ) {
                let name = loop {
                    let candidate = format!("d{next_delay_index}");
                    next_delay_index += 1;
                    if !reserved.contains_key(&candidate) {
                        break candidate;
                    }
                };
                if planned_delay_names
                    .insert(placement.site().clone(), name.clone())
                    .is_some()
                {
                    return Err(application_error(
                        span_for_site(graph, placement.site()),
                        DecompositionErrorKind::PlacementConflict {
                            site: placement.site().clone(),
                            detail: "the decomposition repeats a physical placement site"
                                .to_string(),
                        },
                    ));
                }
                reserved.insert(name, span_for_site(graph, placement.site()));
            }
        }

        let mut lowered_shell = lowered.clone();
        lowered_shell.cell.items.clear();
        Ok(Self {
            graph,
            decomposition,
            original_lowered: lowered.clone(),
            original_provenance: assignment_provenance.to_vec(),
            original_metadata: signal_metadata.to_vec(),
            lowered_shell,
            items,
            signal_metadata: signal_metadata.to_vec(),
            planned_delay_names,
            raw_public_names: BTreeMap::new(),
            next_generated_id: 0,
            facts: AppliedTimingFacts::default(),
        })
    }

    fn span_for_site(&self, site: &PlacementSite) -> Span {
        match site {
            PlacementSite::ExistingAssignment { node, .. }
            | PlacementSite::PublicOutputSplit { signal: node } => self
                .graph
                .node(*node)
                .map(|node| node.span().clone())
                .unwrap_or_else(|| Span::new("<timing-application>", 1, 1)),
            PlacementSite::DependencyEdge {
                dependency_order, ..
            } => self
                .graph
                .dependencies()
                .get(*dependency_order)
                .map(|dependency| dependency.edge().span().clone())
                .unwrap_or_else(|| Span::new("<timing-application>", 1, 1)),
        }
    }

    fn planned_delay_name(&self, site: &PlacementSite) -> Result<String, DecompositionError> {
        self.planned_delay_names
            .get(site)
            .cloned()
            .ok_or_else(|| stale_application_site(self, site))
    }

    fn allocate_generated_id(&mut self) -> AppliedAssignmentId {
        let id = AppliedAssignmentId::TimingGenerated(self.next_generated_id);
        self.next_generated_id += 1;
        id
    }

    fn item_index(&self, id: AppliedAssignmentId) -> Option<usize> {
        self.items.iter().position(
            |item| matches!(item, WorkingItem::Assignment(assignment) if assignment.id == id),
        )
    }

    fn assignment(&self, id: AppliedAssignmentId) -> Option<&WorkingAssignment> {
        let index = self.item_index(id)?;
        match &self.items[index] {
            WorkingItem::Assignment(assignment) => Some(assignment),
            WorkingItem::Other(_) => None,
        }
    }

    fn assignment_mut(&mut self, id: AppliedAssignmentId) -> Option<&mut WorkingAssignment> {
        let index = self.item_index(id)?;
        match &mut self.items[index] {
            WorkingItem::Assignment(assignment) => Some(assignment),
            WorkingItem::Other(_) => None,
        }
    }

    fn insert_assignment(&mut self, index: usize, assignment: WorkingAssignment) {
        self.items
            .insert(index, WorkingItem::Assignment(assignment));
    }

    fn add_delay_signal(
        &mut self,
        name: String,
        span: Span,
        modeled_register: bool,
    ) -> Result<(), DecompositionError> {
        let mut roles = BTreeSet::from([
            TimingSignalRole::Internal,
            TimingSignalRole::TimingTemporary,
        ]);
        if modeled_register {
            roles.insert(TimingSignalRole::ModeledRegister);
        }
        let metadata =
            TimingSignalMetadata::new(name, roles, span.clone()).map_err(|diagnostic| {
                application_error(
                    diagnostic.span,
                    DecompositionErrorKind::PlacementConflict {
                        site: self
                            .decomposition
                            .placements()
                            .first()
                            .expect("application has a placement")
                            .site()
                            .clone(),
                        detail: diagnostic.message,
                    },
                )
            })?;
        self.signal_metadata.push(metadata);
        Ok(())
    }

    fn finish(mut self) -> Result<AppliedTimingTransform, DecompositionError> {
        self.lowered_shell.cell.items = self
            .items
            .iter()
            .map(|item| match item {
                WorkingItem::Other(item) => item.clone(),
                WorkingItem::Assignment(assignment) => {
                    CellItem::Assignment(assignment.assignment.clone())
                }
            })
            .collect();

        let mut final_provenance = Vec::new();
        let mut orders = BTreeMap::new();
        for item in &self.items {
            let WorkingItem::Assignment(assignment) = item else {
                continue;
            };
            let order = final_provenance.len();
            let provenance = AssignmentProvenance::new_with_delay_origin(
                order,
                assignment.provenance.source_assignment_order,
                assignment.provenance.span.clone(),
                assignment.provenance.origin,
                assignment.provenance.delay_origin,
                assignment.provenance.state_controls.clone(),
            )
            .map_err(|diagnostic| {
                application_error(
                    diagnostic.span,
                    DecompositionErrorKind::PlacementConflict {
                        site: self
                            .decomposition
                            .placements()
                            .first()
                            .expect("application has a placement")
                            .site()
                            .clone(),
                        detail: diagnostic.message,
                    },
                )
            })?;
            orders.insert(assignment.id, order);
            final_provenance.push(provenance);
        }
        for placement in &mut self.facts.placements {
            placement.assignment_order = orders[&placement.assignment_id];
        }
        self.facts.assignment_orders = orders.clone();
        self.facts.original_assignment_orders = orders
            .iter()
            .filter_map(|(id, transformed_order)| match id {
                AppliedAssignmentId::Original(original_order) => {
                    Some((*original_order, *transformed_order))
                }
                AppliedAssignmentId::TimingGenerated(_) => None,
            })
            .collect();
        self.lowered_shell.cell.validate().map_err(|error| {
            application_error(
                first_span(&self.signal_metadata, &final_provenance),
                DecompositionErrorKind::PlacementConflict {
                    site: self
                        .decomposition
                        .placements()
                        .first()
                        .expect("application has a placement")
                        .site()
                        .clone(),
                    detail: format!("transformed cell is invalid: {error}"),
                },
            )
        })?;

        let applied_verification = verify_applied_timing_transform(
            &self.lowered_shell,
            &self.signal_metadata,
            &final_provenance,
            self.graph,
            self.decomposition,
            &self.facts,
        )?;
        let verification = ActualDecompositionVerification {
            symbolic: self.decomposition.verification().clone(),
            applied: applied_verification,
            checked_placements: self.facts.placements.clone(),
        };
        let erasure = TimingErasure {
            original: ErasedTimingModel {
                lowered: self.original_lowered,
                assignment_provenance: self.original_provenance,
                signal_metadata: self.original_metadata,
            },
            expected_lowered: self.lowered_shell.clone(),
            expected_provenance: final_provenance.clone(),
            records: self.facts.rewrites.clone(),
            span: first_span(&self.signal_metadata, &final_provenance),
        };
        Ok(AppliedTimingTransform {
            lowered: self.lowered_shell,
            assignment_provenance: final_provenance,
            signal_metadata: self.signal_metadata,
            facts: self.facts,
            verification,
            erasure,
        })
    }
}

/// Rebuilds and independently verifies an already-materialized timing model.
///
/// Generated assignments are identified only by the transform's typed durable
/// IDs and checked against assignment provenance; `dN` spellings are never
/// interpreted.
pub fn verify_applied_timing_transform(
    lowered: &LoweredModule,
    signal_metadata: &[TimingSignalMetadata],
    assignment_provenance: &[AssignmentProvenance],
    original_graph: &TimingGraph,
    decomposition: &Decomposition,
    facts: &AppliedTimingFacts,
) -> Result<AppliedModelVerification, DecompositionError> {
    let assignment_count = lowered
        .cell
        .items
        .iter()
        .filter(|item| matches!(item, CellItem::Assignment(_)))
        .count();
    if facts.assignment_orders.len() != assignment_count
        || assignment_provenance.len() != assignment_count
    {
        return Err(application_error(
            first_span(signal_metadata, assignment_provenance),
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "typed transform assignment identities are not total".to_string(),
            },
        ));
    }

    let mut transformed_orders = BTreeSet::new();
    let mut generated_assignments = BTreeSet::new();
    let mut original_to_transformed = BTreeMap::new();
    for (&id, &order) in &facts.assignment_orders {
        if order >= assignment_count || !transformed_orders.insert(order) {
            return Err(application_error(
                assignment_provenance.get(order).map_or_else(
                    || first_span(signal_metadata, assignment_provenance),
                    |value| value.span().clone(),
                ),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: order,
                    detail: "typed transform assignment orders are duplicated or out of bounds"
                        .to_string(),
                },
            ));
        }
        let provenance = &assignment_provenance[order];
        match id {
            AppliedAssignmentId::Original(original) => {
                if provenance.origin().is_timing_identity() {
                    return Err(application_error(
                        provenance.span().clone(),
                        DecompositionErrorKind::AppliedAssignmentMapping {
                            assignment_order: order,
                            detail: "an original typed ID has timing-identity provenance"
                                .to_string(),
                        },
                    ));
                }
                original_to_transformed.insert(original, order);
            }
            AppliedAssignmentId::TimingGenerated(_) => {
                if !provenance.origin().is_timing_identity() {
                    return Err(application_error(
                        provenance.span().clone(),
                        DecompositionErrorKind::AppliedAssignmentMapping {
                            assignment_order: order,
                            detail: "a generated typed ID lacks timing-identity provenance"
                                .to_string(),
                        },
                    ));
                }
                generated_assignments.insert(order);
            }
        }
    }
    if transformed_orders != (0..assignment_count).collect()
        || original_to_transformed != facts.original_assignment_orders
    {
        return Err(application_error(
            first_span(signal_metadata, assignment_provenance),
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "typed original/generated projections are incomplete or inconsistent"
                    .to_string(),
            },
        ));
    }

    let mut empty_components = BTreeMap::new();
    for placement in &facts.placements {
        let order = *facts
            .assignment_orders
            .get(&placement.assignment_id)
            .ok_or_else(|| {
                application_error(
                    span_for_site(original_graph, &placement.site),
                    DecompositionErrorKind::AppliedAssignmentMapping {
                        assignment_order: placement.assignment_order,
                        detail: "placement assignment has no typed transform identity".to_string(),
                    },
                )
            })?;
        if order != placement.assignment_order
            || empty_components
                .insert(order, placement.empty_components.clone())
                .is_some()
        {
            return Err(application_error(
                assignment_provenance[order].span().clone(),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: order,
                    detail: "placement mapping is stale or repeats an assignment".to_string(),
                },
            ));
        }
    }

    let constraint_sources = original_graph
        .constraints()
        .iter()
        .map(TimingConstraintSource::from_constraint)
        .collect::<Vec<_>>();
    let graph = build_timing_graph(
        &lowered.cell,
        signal_metadata,
        assignment_provenance,
        &constraint_sources,
    )
    .map_err(decomposition_analysis_error)?;
    let cut_graph = cut_register_cycles(&graph).map_err(decomposition_analysis_error)?;
    let report = analyze_timing_graph(&graph, &cut_graph).map_err(decomposition_analysis_error)?;
    verify_applied_model(
        original_graph,
        decomposition,
        &AppliedModelSnapshot::new(
            &graph,
            &cut_graph,
            &report,
            &lowered.cell,
            assignment_provenance,
        ),
        &AppliedVerificationMap::new(
            original_to_transformed,
            generated_assignments,
            empty_components,
        ),
    )
}

pub fn apply_decomposition(
    lowered: &LoweredModule,
    signal_metadata: &[TimingSignalMetadata],
    assignment_provenance: &[AssignmentProvenance],
    graph: &TimingGraph,
    decomposition: &Decomposition,
) -> Result<AppliedTimingTransform, DecompositionError> {
    let mut state = TimingApplicationState::new(
        lowered,
        signal_metadata,
        assignment_provenance,
        graph,
        decomposition,
    )?;
    // A public split establishes the raw name that all other physical sites
    // must address. Names were preallocated in decomposition order, so this
    // dependency-safe application order does not change deterministic naming.
    for placement in decomposition.placements() {
        if matches!(placement.site(), PlacementSite::PublicOutputSplit { .. }) {
            split_public_output(&mut state, placement)?;
        }
    }
    for placement in decomposition.placements() {
        match placement.site() {
            PlacementSite::ExistingAssignment { .. } => {
                apply_existing_assignment(&mut state, placement)?
            }
            PlacementSite::DependencyEdge { .. } => insert_edge_delay(&mut state, placement)?,
            PlacementSite::PublicOutputSplit { .. } => {}
        }
    }
    state.finish()
}

fn apply_existing_assignment(
    state: &mut TimingApplicationState<'_>,
    placement: &DelayPlacement,
) -> Result<(), DecompositionError> {
    let PlacementSite::ExistingAssignment {
        node,
        assignment_order,
    } = placement.site()
    else {
        return Err(application_error(
            state.span_for_site(placement.site()),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "expected an existing-assignment site".to_string(),
            },
        ));
    };
    let node_assignment = assignment_node(state.graph, *node, placement.site())?;
    if node_assignment != *assignment_order {
        return Err(stale_application_site(state, placement.site()));
    }
    let id = AppliedAssignmentId::Original(*assignment_order);
    let delay = materialize_delay(placement.delay(), &state.span_for_site(placement.site()))?;
    if state.assignment(id).is_none() {
        return Err(stale_application_site(state, placement.site()));
    }
    let working = state.assignment_mut(id).expect("assignment was resolved");
    let before = working.assignment.clone();
    if working.provenance.delay_origin.is_intrinsic_source_delay() {
        if working.assignment.delay != delay {
            return Err(application_error(
                working.provenance.span.clone(),
                DecompositionErrorKind::PlacementConflict {
                    site: placement.site().clone(),
                    detail: "an intrinsic source delay differs from the planned placement"
                        .to_string(),
                },
            ));
        }
    } else {
        working.assignment.delay = delay.clone();
        working.provenance.delay_origin = AssignmentDelayOrigin::DecompositionPlacement;
    }
    let after = working.assignment.clone();
    state
        .facts
        .rewrites
        .push(AppliedRewrite::ExistingAssignment {
            assignment: id,
            before,
            after,
        });
    state.facts.placements.push(AppliedPlacement {
        site: placement.site().clone(),
        assignment_id: id,
        assignment_order: 0,
        delay,
        empty_components: placement
            .delay()
            .components()
            .map(<[crate::timing_terms::DelayTerm]>::is_empty)
            .collect(),
    });
    Ok(())
}

/// Inserts a typed `dN = source` identity on one exact functional dependency.
pub fn insert_edge_delay(
    state: &mut TimingApplicationState<'_>,
    placement: &DelayPlacement,
) -> Result<(), DecompositionError> {
    let PlacementSite::DependencyEdge {
        dependency_order,
        source,
        target,
    } = placement.site()
    else {
        return Err(application_error(
            state.span_for_site(placement.site()),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "expected a dependency-edge site".to_string(),
            },
        ));
    };
    let dependency = state
        .graph
        .dependencies()
        .get(*dependency_order)
        .ok_or_else(|| stale_application_site(state, placement.site()))?;
    if dependency.source() != *source || dependency.target() != *target {
        return Err(stale_application_site(state, placement.site()));
    }
    match dependency.edge().kind() {
        DependencyKind::Operand => insert_operand_identity(state, placement, *dependency_order),
        DependencyKind::Drive
        | DependencyKind::StateBoundary
        | DependencyKind::ResolvedNetBoundary => {
            insert_boundary_identity(state, placement, *dependency_order)
        }
        DependencyKind::StateControl => Err(application_error(
            dependency.edge().span().clone(),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "event controls are not value-expression edges".to_string(),
            },
        )),
    }
}

fn insert_operand_identity(
    state: &mut TimingApplicationState<'_>,
    placement: &DelayPlacement,
    dependency_order: usize,
) -> Result<(), DecompositionError> {
    let dependency = &state.graph.dependencies()[dependency_order];
    let source_name = signal_name(state.graph, dependency.source(), placement.site())?.to_string();
    let current_source_name = state
        .raw_public_names
        .get(&source_name)
        .cloned()
        .unwrap_or_else(|| source_name.clone());
    let consumer_order = assignment_node(state.graph, dependency.target(), placement.site())?;
    let operand_index = dependency.edge().operand_index().ok_or_else(|| {
        application_error(
            dependency.edge().span().clone(),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "operand dependency has no operand index".to_string(),
            },
        )
    })?;
    let consumer_id = AppliedAssignmentId::Original(consumer_order);
    let consumer = state
        .assignment(consumer_id)
        .ok_or_else(|| stale_application_site(state, placement.site()))?
        .clone();
    let name = state.planned_delay_name(placement.site())?;
    let delay = materialize_delay(placement.delay(), dependency.edge().span())?;
    let mut after_consumer = consumer.assignment.clone();
    rewrite_exact_operand(
        &mut after_consumer.expr,
        operand_index,
        &current_source_name,
        &name,
        dependency.edge().span(),
        placement.site(),
    )?;
    state
        .assignment_mut(consumer_id)
        .expect("consumer was resolved")
        .assignment = after_consumer.clone();

    let identity_id = state.allocate_generated_id();
    let inserted = Assignment {
        target: name.clone(),
        expr: Expr::atom(current_source_name),
        delay: delay.clone(),
    };
    let inserted_working = WorkingAssignment {
        id: identity_id,
        assignment: inserted.clone(),
        provenance: generated_provenance(&consumer.provenance, dependency.edge().span()),
    };
    let consumer_item_index = state
        .item_index(consumer_id)
        .expect("consumer was resolved");
    state.insert_assignment(consumer_item_index, inserted_working);
    state.add_delay_signal(name, dependency.edge().span().clone(), false)?;
    state
        .facts
        .rewrites
        .push(AppliedRewrite::OperandEdgeIdentity {
            dependency_order,
            consumer: consumer_id,
            identity: identity_id,
            before_consumer: consumer.assignment,
            after_consumer,
            inserted,
        });
    state.facts.placements.push(AppliedPlacement {
        site: placement.site().clone(),
        assignment_id: identity_id,
        assignment_order: 0,
        delay,
        empty_components: placement
            .delay()
            .components()
            .map(<[crate::timing_terms::DelayTerm]>::is_empty)
            .collect(),
    });
    Ok(())
}

fn insert_boundary_identity(
    state: &mut TimingApplicationState<'_>,
    placement: &DelayPlacement,
    dependency_order: usize,
) -> Result<(), DecompositionError> {
    let dependency = &state.graph.dependencies()[dependency_order];
    let driver_order = assignment_node(state.graph, dependency.source(), placement.site())?;
    let target_name = signal_name(state.graph, dependency.target(), placement.site())?.to_string();
    let current_target_name = state
        .raw_public_names
        .get(&target_name)
        .cloned()
        .unwrap_or(target_name);
    let driver_id = AppliedAssignmentId::Original(driver_order);
    let before = state
        .assignment(driver_id)
        .ok_or_else(|| stale_application_site(state, placement.site()))?
        .clone();
    if before.assignment.target != current_target_name {
        return Err(stale_application_site(state, placement.site()));
    }
    let modeled_register = dependency.edge().kind() == DependencyKind::StateBoundary;
    let name = state.planned_delay_name(placement.site())?;
    let delay = materialize_delay(placement.delay(), dependency.edge().span())?;
    let mut after = before.assignment.clone();
    after.target = name.clone();
    state
        .assignment_mut(driver_id)
        .expect("driver was resolved")
        .assignment = after.clone();
    if modeled_register {
        transfer_register(&mut state.lowered_shell, &current_target_name, &name);
        transfer_register_role(
            &mut state.signal_metadata,
            &current_target_name,
            dependency.edge().span(),
        )?;
    }
    state.add_delay_signal(
        name.clone(),
        dependency.edge().span().clone(),
        modeled_register,
    )?;

    let identity_id = state.allocate_generated_id();
    let inserted = Assignment {
        target: current_target_name,
        expr: Expr::atom(name),
        delay: delay.clone(),
    };
    let inserted_working = WorkingAssignment {
        id: identity_id,
        assignment: inserted.clone(),
        provenance: generated_provenance(&before.provenance, dependency.edge().span()),
    };
    let driver_item_index = state.item_index(driver_id).expect("driver was resolved");
    state.insert_assignment(driver_item_index + 1, inserted_working);
    state
        .facts
        .rewrites
        .push(AppliedRewrite::BoundaryEdgeIdentity {
            dependency_order,
            driver: driver_id,
            identity: identity_id,
            before_driver: before.assignment,
            after_driver: after,
            inserted,
        });
    state.facts.placements.push(AppliedPlacement {
        site: placement.site().clone(),
        assignment_id: identity_id,
        assignment_order: 0,
        delay,
        empty_components: placement
            .delay()
            .components()
            .map(<[crate::timing_terms::DelayTerm]>::is_empty)
            .collect(),
    });
    Ok(())
}

/// Splits one eligible single-driver output into raw `dN` state/value and a
/// public identity carrying the selected delay.
pub fn split_public_output(
    state: &mut TimingApplicationState<'_>,
    placement: &DelayPlacement,
) -> Result<(), DecompositionError> {
    let PlacementSite::PublicOutputSplit { signal } = placement.site() else {
        return Err(application_error(
            state.span_for_site(placement.site()),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "expected a public-output split site".to_string(),
            },
        ));
    };
    let node = state
        .graph
        .node(*signal)
        .ok_or_else(|| stale_application_site(state, placement.site()))?;
    let TimingNodeKind::Signal(signal_node) = node.kind() else {
        return Err(stale_application_site(state, placement.site()));
    };
    let signal_name = signal_node.name().to_string();
    if signal_node.has_role(TimingSignalRole::Inout) {
        return Err(application_error(
            node.span().clone(),
            DecompositionErrorKind::PublicSplitInout {
                signal: signal_name,
            },
        ));
    }
    if !signal_node.has_role(TimingSignalRole::Output) {
        return Err(application_error(
            node.span().clone(),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "signal is not a public output".to_string(),
            },
        ));
    }
    if !state.graph.dependencies().iter().any(|dependency| {
        dependency.source() == *signal && dependency.edge().kind() == DependencyKind::Operand
    }) {
        return Err(application_error(
            node.span().clone(),
            DecompositionErrorKind::UnsupportedPlacement {
                site: placement.site().clone(),
                detail: "public output is not read internally and is not a split candidate"
                    .to_string(),
            },
        ));
    }
    let drivers = state
        .items
        .iter()
        .filter_map(|item| match item {
            WorkingItem::Assignment(assignment)
                if matches!(assignment.id, AppliedAssignmentId::Original(_))
                    && assignment.assignment.target == signal_name =>
            {
                Some(assignment.id)
            }
            WorkingItem::Other(_) | WorkingItem::Assignment(_) => None,
        })
        .collect::<Vec<_>>();
    if drivers.len() != 1 {
        return Err(application_error(
            node.span().clone(),
            DecompositionErrorKind::PublicSplitDriverCount {
                signal: signal_name,
                drivers: drivers.len(),
            },
        ));
    }
    let driver_id = drivers[0];
    let before = state.assignment(driver_id).expect("driver exists").clone();
    let raw_name = state.planned_delay_name(placement.site())?;
    let delay = materialize_delay(placement.delay(), node.span())?;

    // Only source/baseline assignments are rewritten. No generated timing
    // identity exists yet, so the public identity cannot accidentally become
    // self-referential.
    for item in &mut state.items {
        if let WorkingItem::Assignment(assignment) = item
            && matches!(assignment.id, AppliedAssignmentId::Original(_))
        {
            rewrite_all_atoms(&mut assignment.assignment.expr, &signal_name, &raw_name);
        }
    }
    let driver = state.assignment_mut(driver_id).expect("driver exists");
    driver.assignment.target = raw_name.clone();
    let after = driver.assignment.clone();

    let modeled_register = signal_node.has_role(TimingSignalRole::ModeledRegister);
    if modeled_register {
        transfer_register(&mut state.lowered_shell, &signal_name, &raw_name);
        transfer_register_role(&mut state.signal_metadata, &signal_name, node.span())?;
    }
    state.add_delay_signal(raw_name.clone(), node.span().clone(), modeled_register)?;
    state
        .raw_public_names
        .insert(signal_name.clone(), raw_name.clone());

    let identity_id = state.allocate_generated_id();
    let inserted = Assignment {
        target: signal_name.clone(),
        expr: Expr::atom(raw_name),
        delay: delay.clone(),
    };
    let inserted_working = WorkingAssignment {
        id: identity_id,
        assignment: inserted.clone(),
        provenance: generated_provenance(&before.provenance, node.span()),
    };
    let driver_item_index = state.item_index(driver_id).expect("driver exists");
    state.insert_assignment(driver_item_index + 1, inserted_working);
    state
        .facts
        .rewrites
        .push(AppliedRewrite::PublicOutputSplit {
            signal: signal_name,
            driver: driver_id,
            identity: identity_id,
            before_driver: before.assignment,
            after_driver: after,
            inserted,
        });
    state.facts.placements.push(AppliedPlacement {
        site: placement.site().clone(),
        assignment_id: identity_id,
        assignment_order: 0,
        delay,
        empty_components: placement
            .delay()
            .components()
            .map(<[crate::timing_terms::DelayTerm]>::is_empty)
            .collect(),
    });
    Ok(())
}

fn generated_provenance(parent: &WorkingProvenance, span: &Span) -> WorkingProvenance {
    WorkingProvenance {
        source_assignment_order: parent.source_assignment_order,
        span: span.clone(),
        origin: AssignmentOrigin::GeneratedTimingIdentity {
            parent: parent.origin.source(),
        },
        delay_origin: AssignmentDelayOrigin::DecompositionPlacement,
        state_controls: Vec::new(),
    }
}

fn materialize_delay(
    delay: &PlacementDelay,
    span: &Span,
) -> Result<DelayTuple, DecompositionError> {
    let component = |index| {
        delay
            .canonical_component(index)
            .map_err(|error| {
                application_error(
                    span.clone(),
                    DecompositionErrorKind::SymbolicTerms {
                        detail: error.to_string(),
                    },
                )
            })?
            .map_or_else(
                || {
                    TimingExpr::atom("0").map_err(|error| {
                        application_error(
                            span.clone(),
                            DecompositionErrorKind::SymbolicTerms {
                                detail: error.to_string(),
                            },
                        )
                    })
                },
                Ok,
            )
    };
    match delay {
        PlacementDelay::One(_) => Ok(DelayTuple::One(component(0)?)),
        PlacementDelay::Two { .. } => Ok(DelayTuple::Two {
            rise: component(0)?,
            fall: component(1)?,
        }),
        PlacementDelay::Three { .. } => Ok(DelayTuple::Three {
            rise: component(0)?,
            fall: component(1)?,
            turn_off: component(2)?,
        }),
    }
}

fn rewrite_exact_operand(
    expr: &mut Expr,
    operand_index: usize,
    expected: &str,
    replacement: &str,
    span: &Span,
    site: &PlacementSite,
) -> Result<(), DecompositionError> {
    let operand = match expr {
        Expr::Atom(_) if operand_index == 0 => expr,
        Expr::List(items) => items.get_mut(operand_index + 1).ok_or_else(|| {
            application_error(
                span.clone(),
                DecompositionErrorKind::PlacementConflict {
                    site: site.clone(),
                    detail: format!("operand index {operand_index} is absent"),
                },
            )
        })?,
        Expr::Atom(_) => {
            return Err(application_error(
                span.clone(),
                DecompositionErrorKind::PlacementConflict {
                    site: site.clone(),
                    detail: format!("direct atom has no operand index {operand_index}"),
                },
            ));
        }
    };
    match operand {
        Expr::Atom(atom) if atom == expected => {
            *atom = replacement.to_string();
            Ok(())
        }
        Expr::Atom(atom) => Err(application_error(
            span.clone(),
            DecompositionErrorKind::PlacementConflict {
                site: site.clone(),
                detail: format!("operand is `{atom}`, expected `{expected}`"),
            },
        )),
        Expr::List(_) => Err(application_error(
            span.clone(),
            DecompositionErrorKind::PlacementConflict {
                site: site.clone(),
                detail: "lowered operand is not flat".to_string(),
            },
        )),
    }
}

fn rewrite_all_atoms(expr: &mut Expr, expected: &str, replacement: &str) {
    match expr {
        Expr::Atom(atom) => {
            if atom == expected {
                *atom = replacement.to_string();
            }
        }
        Expr::List(items) => {
            for item in items.iter_mut().skip(1) {
                rewrite_all_atoms(item, expected, replacement);
            }
        }
    }
}

fn transfer_register(lowered: &mut LoweredModule, old: &str, new: &str) {
    if let Some(register) = lowered
        .cell
        .registers
        .iter_mut()
        .find(|register| register.name == old)
    {
        register.name = new.to_string();
    }
}

fn transfer_register_role(
    metadata: &mut [TimingSignalMetadata],
    signal: &str,
    span: &Span,
) -> Result<(), DecompositionError> {
    let Some(index) = metadata.iter().position(|entry| entry.name() == signal) else {
        return Err(application_error(
            span.clone(),
            DecompositionErrorKind::ErasureMismatch {
                detail: format!("missing signal metadata for register `{signal}`"),
            },
        ));
    };
    let mut roles = metadata[index].roles().clone();
    roles.remove(&TimingSignalRole::ModeledRegister);
    metadata[index] =
        TimingSignalMetadata::new(signal.to_string(), roles, metadata[index].span().clone())
            .map_err(|diagnostic| {
                application_error(
                    diagnostic.span,
                    DecompositionErrorKind::ErasureMismatch {
                        detail: diagnostic.message,
                    },
                )
            })?;
    Ok(())
}

fn assignment_node(
    graph: &TimingGraph,
    node: TimingNodeId,
    site: &PlacementSite,
) -> Result<usize, DecompositionError> {
    match graph.node(node).map(|node| node.kind()) {
        Some(TimingNodeKind::Assignment(assignment)) => Ok(assignment.assignment_order()),
        _ => Err(stale_application_site_from_graph(graph, site)),
    }
}

fn signal_name<'a>(
    graph: &'a TimingGraph,
    node: TimingNodeId,
    site: &PlacementSite,
) -> Result<&'a str, DecompositionError> {
    match graph.node(node).map(|node| node.kind()) {
        Some(TimingNodeKind::Signal(signal)) => Ok(signal.name()),
        _ => Err(stale_application_site_from_graph(graph, site)),
    }
}

fn stale_application_site(
    state: &TimingApplicationState<'_>,
    site: &PlacementSite,
) -> DecompositionError {
    application_error(
        state.span_for_site(site),
        DecompositionErrorKind::StalePlacementSite { site: site.clone() },
    )
}

fn stale_application_site_from_graph(
    graph: &TimingGraph,
    site: &PlacementSite,
) -> DecompositionError {
    application_error(
        span_for_site(graph, site),
        DecompositionErrorKind::StalePlacementSite { site: site.clone() },
    )
}

fn span_for_site(graph: &TimingGraph, site: &PlacementSite) -> Span {
    match site {
        PlacementSite::ExistingAssignment { node, .. }
        | PlacementSite::PublicOutputSplit { signal: node } => graph
            .node(*node)
            .map(|node| node.span().clone())
            .unwrap_or_else(|| Span::new("<timing-application>", 1, 1)),
        PlacementSite::DependencyEdge {
            dependency_order, ..
        } => graph
            .dependencies()
            .get(*dependency_order)
            .map(|dependency| dependency.edge().span().clone())
            .unwrap_or_else(|| Span::new("<timing-application>", 1, 1)),
    }
}

fn application_error(span: Span, kind: DecompositionErrorKind) -> DecompositionError {
    DecompositionError::new(span, kind)
}

fn decomposition_analysis_error(diagnostic: crate::diagnostic::Diagnostic) -> DecompositionError {
    DecompositionError::new(
        diagnostic.span,
        DecompositionErrorKind::InconsistentAnalysis {
            detail: diagnostic.message,
        },
    )
}

fn first_span(metadata: &[TimingSignalMetadata], provenance: &[AssignmentProvenance]) -> Span {
    provenance
        .first()
        .map(|value| value.span().clone())
        .or_else(|| metadata.first().map(|value| value.span().clone()))
        .unwrap_or_else(|| Span::new("<timing-application>", 1, 1))
}

fn reserve_lowered_names(
    lowered: &LoweredModule,
    graph: &TimingGraph,
    metadata: &[TimingSignalMetadata],
    provenance: &[AssignmentProvenance],
    reserved: &mut BTreeMap<String, Span>,
) {
    let default = first_span(metadata, provenance);
    for name in lowered
        .cell
        .inputs
        .iter()
        .chain(&lowered.cell.outputs)
        .chain(lowered.cell.registers.iter().map(|register| &register.name))
    {
        reserved.entry(name.clone()).or_insert_with(|| {
            graph
                .signal_id(name)
                .and_then(|node| graph.node(node))
                .map(|node| node.span().clone())
                .unwrap_or_else(|| default.clone())
        });
    }
    let mut order = 0;
    for item in &lowered.cell.items {
        if let CellItem::Assignment(assignment) = item {
            let span = provenance
                .get(order)
                .map(|value| value.span().clone())
                .unwrap_or_else(|| default.clone());
            reserved
                .entry(assignment.target.clone())
                .or_insert_with(|| span.clone());
            reserve_expr_atoms(&assignment.expr, &span, reserved);
            for component in assignment.delay.components() {
                reserve_expr_atoms(component.as_expr(), &span, reserved);
            }
            order += 1;
        }
    }
    for node in graph.nodes() {
        match node.kind() {
            TimingNodeKind::Signal(signal) => {
                reserved
                    .entry(signal.name().to_string())
                    .or_insert_with(|| node.span().clone());
            }
            TimingNodeKind::Assignment(assignment) => {
                reserved
                    .entry(assignment.target().to_string())
                    .or_insert_with(|| node.span().clone());
            }
        }
    }
    for (name, expression) in &lowered.timing_aliases {
        reserved
            .entry(name.clone())
            .or_insert_with(|| default.clone());
        reserve_expr_atoms(expression.as_expr(), &default, reserved);
    }
}

fn reserve_expr_atoms(expr: &Expr, span: &Span, reserved: &mut BTreeMap<String, Span>) {
    match expr {
        Expr::Atom(atom) => {
            reserved.entry(atom.clone()).or_insert_with(|| span.clone());
        }
        Expr::List(items) => {
            for item in items {
                reserve_expr_atoms(item, span, reserved);
            }
        }
    }
}

#[allow(dead_code)]
const fn _source_origin_is_copy(value: SourceAssignmentOrigin) -> SourceAssignmentOrigin {
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Cell, LogicValue, Register, TimingOperator, ValueOperator};
    use crate::timing_decompose::{PlacementDelay, decompose_timing};
    use crate::timing_graph::{
        TimingControlSource, TimingSignalRole, Transition, build_functional_timing_graph,
    };
    use crate::timing_terms::DelayTerm;

    fn span(line: usize) -> Span {
        Span::new("apply.sv", line, 1)
    }

    fn timing(value: &str) -> TimingExpr {
        TimingExpr::atom(value).unwrap()
    }

    fn tuple(value: &str) -> DelayTuple {
        DelayTuple::One(timing(value))
    }

    fn placement_delay(value: &str) -> PlacementDelay {
        PlacementDelay::One(vec![DelayTerm::from_timing_expr(timing(value)).unwrap()])
    }

    fn sum_tuple(values: &[&str]) -> DelayTuple {
        DelayTuple::One(
            TimingExpr::operation(
                TimingOperator::Add,
                values.iter().map(|value| timing(value)).collect(),
            )
            .unwrap(),
        )
    }

    fn metadata(name: &str, roles: &[TimingSignalRole], line: usize) -> TimingSignalMetadata {
        TimingSignalMetadata::new(
            name.to_string(),
            roles.iter().copied().collect(),
            span(line),
        )
        .unwrap()
    }

    fn provenance(
        order: usize,
        origin: SourceAssignmentOrigin,
        delay_origin: AssignmentDelayOrigin,
        line: usize,
    ) -> AssignmentProvenance {
        AssignmentProvenance::new_with_delay_origin(
            order,
            order,
            span(line),
            AssignmentOrigin::Source(origin),
            delay_origin,
            Vec::new(),
        )
        .unwrap()
    }

    fn stateful_provenance(order: usize, line: usize) -> AssignmentProvenance {
        AssignmentProvenance::new_with_delay_origin(
            order,
            order,
            span(line),
            AssignmentOrigin::Source(SourceAssignmentOrigin::ProceduralStateful),
            AssignmentDelayOrigin::ImplicitZero,
            vec![StateControlProvenance::new(
                "clk".to_string(),
                Some(crate::timing_graph::Transition::Rise),
                span(line),
            )],
        )
        .unwrap()
    }

    fn lowered(
        inputs: &[&str],
        outputs: &[&str],
        registers: Vec<Register>,
        assignments: Vec<Assignment>,
    ) -> LoweredModule {
        LoweredModule {
            cell: Cell {
                name: "sample".to_string(),
                inputs: inputs.iter().map(|value| (*value).to_string()).collect(),
                outputs: outputs.iter().map(|value| (*value).to_string()).collect(),
                registers,
                parameters: Vec::new(),
                items: assignments.into_iter().map(CellItem::Assignment).collect(),
            },
            timing_aliases: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn assignments(items: &[WorkingItem]) -> Vec<&WorkingAssignment> {
        items
            .iter()
            .filter_map(|item| match item {
                WorkingItem::Assignment(assignment) => Some(assignment),
                WorkingItem::Other(_) => None,
            })
            .collect()
    }

    fn cell_assignments(lowered: &LoweredModule) -> Vec<&Assignment> {
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

    #[test]
    fn operand_identity_rewrites_only_the_selected_repeated_occurrence() {
        let lowered = lowered(
            &["a"],
            &["y"],
            Vec::new(),
            vec![Assignment {
                target: "y".to_string(),
                expr: Expr::value(ValueOperator::Or, vec![Expr::atom("a"), Expr::atom("a")]),
                delay: tuple("0"),
            }],
        );
        let metadata = vec![
            metadata("a", &[TimingSignalRole::Input], 1),
            metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![provenance(
            0,
            SourceAssignmentOrigin::Continuous,
            AssignmentDelayOrigin::ImplicitZero,
            3,
        )];
        let graph = build_functional_timing_graph(&lowered.cell, &metadata, &provenance).unwrap();
        let dependency = &graph.dependencies()[1];
        assert_eq!(dependency.edge().operand_index(), Some(1));
        let placement = DelayPlacement::test_only(
            PlacementSite::DependencyEdge {
                dependency_order: 1,
                source: dependency.source(),
                target: dependency.target(),
            },
            placement_delay("T"),
        );
        let decomposition = Decomposition::test_only(vec![placement.clone()]);
        let mut state =
            TimingApplicationState::new(&lowered, &metadata, &provenance, &graph, &decomposition)
                .unwrap();

        insert_edge_delay(&mut state, &placement).unwrap();
        let values = assignments(&state.items);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].assignment.target, "d0");
        assert_eq!(values[0].assignment.expr, Expr::atom("a"));
        assert_eq!(values[0].assignment.delay, tuple("T"));
        assert_eq!(
            values[1].assignment.expr,
            Expr::value(ValueOperator::Or, vec![Expr::atom("a"), Expr::atom("d0")])
        );
    }

    #[test]
    fn state_boundary_moves_register_initial_and_controls_to_raw_state() {
        let lowered = lowered(
            &["d", "clk"],
            &["q"],
            vec![Register {
                name: "q".to_string(),
                initial: LogicValue::One,
            }],
            vec![Assignment {
                target: "q".to_string(),
                expr: Expr::atom("d"),
                delay: tuple("0"),
            }],
        );
        let metadata = vec![
            metadata("d", &[TimingSignalRole::Input], 1),
            metadata("clk", &[TimingSignalRole::Input], 1),
            metadata(
                "q",
                &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
                2,
            ),
        ];
        let provenance = vec![stateful_provenance(0, 3)];
        let graph = build_functional_timing_graph(&lowered.cell, &metadata, &provenance).unwrap();
        let boundary_order = graph
            .dependencies()
            .iter()
            .position(|dependency| dependency.edge().kind() == DependencyKind::StateBoundary)
            .unwrap();
        let dependency = &graph.dependencies()[boundary_order];
        let placement = DelayPlacement::test_only(
            PlacementSite::DependencyEdge {
                dependency_order: boundary_order,
                source: dependency.source(),
                target: dependency.target(),
            },
            placement_delay("Tq"),
        );
        let decomposition = Decomposition::test_only(vec![placement.clone()]);
        let mut state =
            TimingApplicationState::new(&lowered, &metadata, &provenance, &graph, &decomposition)
                .unwrap();

        insert_edge_delay(&mut state, &placement).unwrap();
        assert_eq!(
            state.lowered_shell.cell.registers,
            vec![Register {
                name: "d0".to_string(),
                initial: LogicValue::One
            }]
        );
        let values = assignments(&state.items);
        assert_eq!(values[0].assignment.target, "d0");
        assert_eq!(values[0].assignment.expr, Expr::atom("d"));
        assert_eq!(values[0].provenance.origin, provenance[0].origin());
        assert_eq!(
            values[0].provenance.state_controls,
            provenance[0].state_controls()
        );
        assert_eq!(values[1].assignment.target, "q");
        assert_eq!(values[1].assignment.expr, Expr::atom("d0"));
        assert_eq!(values[1].assignment.delay, tuple("Tq"));
        assert!(values[1].provenance.state_controls.is_empty());
        assert!(
            state
                .signal_metadata
                .iter()
                .find(|entry| entry.name() == "d0")
                .unwrap()
                .roles()
                .contains(&TimingSignalRole::ModeledRegister)
        );
        assert!(
            !state
                .signal_metadata
                .iter()
                .find(|entry| entry.name() == "q")
                .unwrap()
                .roles()
                .contains(&TimingSignalRole::ModeledRegister)
        );
    }

    #[test]
    fn public_split_rewrites_feedback_and_internal_reads_and_moves_state() {
        let lowered = lowered(
            &["c", "d", "clk"],
            &["q", "z"],
            vec![Register {
                name: "q".to_string(),
                initial: LogicValue::Zero,
            }],
            vec![
                Assignment {
                    target: "q".to_string(),
                    expr: Expr::value(
                        ValueOperator::Mux,
                        vec![Expr::atom("c"), Expr::atom("d"), Expr::atom("q")],
                    ),
                    delay: tuple("0"),
                },
                Assignment {
                    target: "z".to_string(),
                    expr: Expr::value(ValueOperator::Not, vec![Expr::atom("q")]),
                    delay: tuple("0"),
                },
            ],
        );
        let metadata = vec![
            metadata("c", &[TimingSignalRole::Input], 1),
            metadata("d", &[TimingSignalRole::Input], 1),
            metadata("clk", &[TimingSignalRole::Input], 1),
            metadata(
                "q",
                &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
                2,
            ),
            metadata("z", &[TimingSignalRole::Output], 3),
        ];
        let provenance = vec![
            stateful_provenance(0, 4),
            provenance(
                1,
                SourceAssignmentOrigin::Continuous,
                AssignmentDelayOrigin::ImplicitZero,
                5,
            ),
        ];
        let graph = build_functional_timing_graph(&lowered.cell, &metadata, &provenance).unwrap();
        let q = graph.signal_id("q").unwrap();
        let placement = DelayPlacement::test_only(
            PlacementSite::PublicOutputSplit { signal: q },
            placement_delay("Tq"),
        );
        let decomposition = Decomposition::test_only(vec![placement.clone()]);
        let mut state =
            TimingApplicationState::new(&lowered, &metadata, &provenance, &graph, &decomposition)
                .unwrap();

        split_public_output(&mut state, &placement).unwrap();
        let values = assignments(&state.items);
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].assignment.target, "d0");
        assert_eq!(
            values[0].assignment.expr,
            Expr::value(
                ValueOperator::Mux,
                vec![Expr::atom("c"), Expr::atom("d"), Expr::atom("d0")]
            )
        );
        assert_eq!(
            values[0].provenance.state_controls,
            provenance[0].state_controls()
        );
        assert_eq!(values[1].assignment.target, "q");
        assert_eq!(values[1].assignment.expr, Expr::atom("d0"));
        assert_eq!(values[1].assignment.delay, tuple("Tq"));
        assert!(values[1].provenance.state_controls.is_empty());
        assert_eq!(
            values[2].assignment.expr,
            Expr::value(ValueOperator::Not, vec![Expr::atom("d0")])
        );
        assert_eq!(state.lowered_shell.cell.registers[0].name, "d0");
    }

    #[test]
    fn deterministic_delay_names_skip_reserved_indices() {
        let lowered = lowered(
            &["a", "d0"],
            &["y"],
            Vec::new(),
            vec![Assignment {
                target: "y".to_string(),
                expr: Expr::value(ValueOperator::Or, vec![Expr::atom("a"), Expr::atom("d0")]),
                delay: tuple("0"),
            }],
        );
        let metadata = vec![
            metadata("a", &[TimingSignalRole::Input], 1),
            metadata("d0", &[TimingSignalRole::Input], 7),
            metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![provenance(
            0,
            SourceAssignmentOrigin::Continuous,
            AssignmentDelayOrigin::ImplicitZero,
            3,
        )];
        let graph = build_functional_timing_graph(&lowered.cell, &metadata, &provenance).unwrap();
        let placements = (0..2)
            .map(|order| {
                let dependency = &graph.dependencies()[order];
                DelayPlacement::test_only(
                    PlacementSite::DependencyEdge {
                        dependency_order: order,
                        source: dependency.source(),
                        target: dependency.target(),
                    },
                    placement_delay("T"),
                )
            })
            .collect::<Vec<_>>();
        let decomposition = Decomposition::test_only(placements.clone());
        let mut state =
            TimingApplicationState::new(&lowered, &metadata, &provenance, &graph, &decomposition)
                .unwrap();

        for placement in &placements {
            insert_edge_delay(&mut state, placement).unwrap();
        }
        let values = assignments(&state.items);
        let identities = values
            .iter()
            .filter(|value| value.assignment.target != "y")
            .map(|value| {
                (
                    value.assignment.target.as_str(),
                    value.assignment.expr.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            vec![("d1", Expr::atom("a")), ("d2", Expr::atom("d0")),]
        );
        assert_eq!(
            values
                .iter()
                .find(|value| value.assignment.target == "y")
                .unwrap()
                .assignment
                .expr,
            Expr::value(ValueOperator::Or, vec![Expr::atom("d1"), Expr::atom("d2")])
        );
    }

    #[test]
    fn existing_intrinsic_delay_must_match_the_planned_value_exactly() {
        let lowered = lowered(
            &["a"],
            &["y"],
            Vec::new(),
            vec![Assignment {
                target: "y".to_string(),
                expr: Expr::atom("a"),
                delay: tuple("source"),
            }],
        );
        let metadata = vec![
            metadata("a", &[TimingSignalRole::Input], 1),
            metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![provenance(
            0,
            SourceAssignmentOrigin::Continuous,
            AssignmentDelayOrigin::ExplicitSourceDelay,
            3,
        )];
        let graph = build_functional_timing_graph(&lowered.cell, &metadata, &provenance).unwrap();
        let assignment_node = graph.assignment_id(0).unwrap();
        let placement = DelayPlacement::test_only(
            PlacementSite::ExistingAssignment {
                node: assignment_node,
                assignment_order: 0,
            },
            placement_delay("planned"),
        );
        let decomposition = Decomposition::test_only(vec![placement.clone()]);
        let mut state =
            TimingApplicationState::new(&lowered, &metadata, &provenance, &graph, &decomposition)
                .unwrap();

        let error = apply_existing_assignment(&mut state, &placement).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::PlacementConflict { .. }
        ));
        assert_eq!(
            assignments(&state.items)[0].assignment.delay,
            tuple("source")
        );
    }

    fn applied_operand_transform() -> (AppliedTimingTransform, TimingGraph, Decomposition) {
        let lowered = lowered(
            &["a"],
            &["y"],
            Vec::new(),
            vec![Assignment {
                target: "y".to_string(),
                expr: Expr::atom("a"),
                delay: tuple("0"),
            }],
        );
        let metadata = vec![
            metadata("a", &[TimingSignalRole::Input], 1),
            metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![provenance(
            0,
            SourceAssignmentOrigin::Continuous,
            AssignmentDelayOrigin::ImplicitZero,
            3,
        )];
        let constraint = TimingConstraintSource::new(
            0,
            vec![TimingControlSource::new("a", None, span(4)).unwrap()],
            "y",
            tuple("T"),
            span(4),
        )
        .unwrap();
        let graph =
            build_timing_graph(&lowered.cell, &metadata, &provenance, &[constraint]).unwrap();
        let cut = cut_register_cycles(&graph).unwrap();
        let report = analyze_timing_graph(&graph, &cut).unwrap();
        let decomposition = decompose_timing(&graph, &cut, &report).unwrap();
        let applied =
            apply_decomposition(&lowered, &metadata, &provenance, &graph, &decomposition).unwrap();
        (applied, graph, decomposition)
    }

    #[test]
    fn actual_verifier_reads_the_transformed_assignment_delay_tuple() {
        let (mut applied, graph, decomposition) = applied_operand_transform();
        let order = applied.facts.placements[0].assignment_order;
        let assignment = applied
            .lowered
            .cell
            .items
            .iter_mut()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some(assignment),
                CellItem::Blank | CellItem::Comment(_) => None,
            })
            .nth(order)
            .unwrap();
        assignment.delay = tuple("corrupt");

        let error = verify_applied_timing_transform(
            &applied.lowered,
            &applied.signal_metadata,
            &applied.assignment_provenance,
            &graph,
            &decomposition,
            &applied.facts,
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::AppliedPathReconstructionMismatch { .. }
        ));
    }

    fn applied_compound_state_transform() -> (
        AppliedTimingTransform,
        LoweredModule,
        Vec<TimingSignalMetadata>,
        Vec<AssignmentProvenance>,
        TimingGraph,
        Decomposition,
    ) {
        let lowered = lowered(
            &["c", "d", "clk"],
            &["q", "z"],
            vec![Register {
                name: "q".to_string(),
                initial: LogicValue::One,
            }],
            vec![
                Assignment {
                    target: "q".to_string(),
                    expr: Expr::value(
                        ValueOperator::Mux,
                        vec![Expr::atom("c"), Expr::atom("d"), Expr::atom("q")],
                    ),
                    delay: tuple("0"),
                },
                Assignment {
                    target: "z".to_string(),
                    expr: Expr::value(ValueOperator::Not, vec![Expr::atom("q")]),
                    delay: tuple("0"),
                },
            ],
        );
        let metadata = vec![
            metadata("c", &[TimingSignalRole::Input], 1),
            metadata("d", &[TimingSignalRole::Input], 1),
            metadata("clk", &[TimingSignalRole::Input], 1),
            metadata(
                "q",
                &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
                2,
            ),
            metadata("z", &[TimingSignalRole::Output], 3),
        ];
        let provenance = vec![
            stateful_provenance(0, 4),
            provenance(
                1,
                SourceAssignmentOrigin::Continuous,
                AssignmentDelayOrigin::ImplicitZero,
                5,
            ),
        ];
        let constraint = TimingConstraintSource::new(
            0,
            vec![TimingControlSource::new("clk", Some(Transition::Rise), span(6)).unwrap()],
            "q",
            sum_tuple(&["E", "A", "Q"]),
            span(6),
        )
        .unwrap();
        let graph =
            build_timing_graph(&lowered.cell, &metadata, &provenance, &[constraint]).unwrap();
        let cut = cut_register_cycles(&graph).unwrap();
        let report = analyze_timing_graph(&graph, &cut).unwrap();
        let pure = decompose_timing(&graph, &cut, &report).unwrap();

        let assignment = graph.assignment_id(0).unwrap();
        let boundary_order = graph
            .dependencies()
            .iter()
            .position(|dependency| {
                dependency.source() == assignment
                    && dependency.edge().kind() == DependencyKind::StateBoundary
            })
            .unwrap();
        let boundary = &graph.dependencies()[boundary_order];
        let q = graph.signal_id("q").unwrap();
        assert_eq!(boundary.target(), q);
        let decomposition = pure.test_only_replacing_placements(vec![
            DelayPlacement::test_only(
                PlacementSite::ExistingAssignment {
                    node: assignment,
                    assignment_order: 0,
                },
                placement_delay("E"),
            ),
            DelayPlacement::test_only(
                PlacementSite::DependencyEdge {
                    dependency_order: boundary_order,
                    source: boundary.source(),
                    target: boundary.target(),
                },
                placement_delay("A"),
            ),
            DelayPlacement::test_only(
                PlacementSite::PublicOutputSplit { signal: q },
                placement_delay("Q"),
            ),
        ]);
        let applied =
            apply_decomposition(&lowered, &metadata, &provenance, &graph, &decomposition).unwrap();
        (applied, lowered, metadata, provenance, graph, decomposition)
    }

    #[test]
    fn existing_boundary_and_public_delays_preserve_one_state_epoch_exactly() {
        let (applied, original, metadata, provenance, original_graph, _) =
            applied_compound_state_transform();
        assert_eq!(
            applied.lowered.cell.registers,
            vec![Register {
                name: "d0".to_string(),
                initial: LogicValue::One,
            }]
        );

        let original_order = applied.facts.assignment_orders[&AppliedAssignmentId::Original(0)];
        let boundary = applied
            .facts
            .placements
            .iter()
            .find(|placement| {
                matches!(
                    placement.site,
                    PlacementSite::DependencyEdge { dependency_order, .. }
                        if original_graph.dependencies()[dependency_order].edge().kind()
                            == DependencyKind::StateBoundary
                )
            })
            .unwrap();
        let public = applied
            .facts
            .placements
            .iter()
            .find(|placement| matches!(placement.site, PlacementSite::PublicOutputSplit { .. }))
            .unwrap();
        let boundary_order = boundary.assignment_order;
        let public_order = public.assignment_order;
        assert_eq!(
            applied.assignment_provenance[original_order].state_controls(),
            provenance[0].state_controls()
        );
        assert!(
            applied.assignment_provenance[boundary_order]
                .state_controls()
                .is_empty()
        );
        assert!(
            applied.assignment_provenance[public_order]
                .state_controls()
                .is_empty()
        );

        let values = cell_assignments(&applied.lowered);
        assert_eq!(values[original_order].target, "d0");
        assert_eq!(values[original_order].delay, tuple("E"));
        assert_eq!(
            values[original_order].expr,
            Expr::value(
                ValueOperator::Mux,
                vec![Expr::atom("c"), Expr::atom("d"), Expr::atom("d1")]
            )
        );
        assert_eq!(values[boundary_order].target, "d1");
        assert_eq!(values[boundary_order].expr, Expr::atom("d0"));
        assert_eq!(values[boundary_order].delay, tuple("A"));
        assert_eq!(values[public_order].target, "q");
        assert_eq!(values[public_order].expr, Expr::atom("d1"));
        assert_eq!(values[public_order].delay, tuple("Q"));
        let derived = values
            .iter()
            .find(|assignment| assignment.target == "z")
            .unwrap();
        assert_eq!(
            derived.expr,
            Expr::value(ValueOperator::Not, vec![Expr::atom("d1")])
        );

        let sources = original_graph
            .constraints()
            .iter()
            .map(TimingConstraintSource::from_constraint)
            .collect::<Vec<_>>();
        let rebuilt = build_timing_graph(
            &applied.lowered.cell,
            &applied.signal_metadata,
            &applied.assignment_provenance,
            &sources,
        )
        .unwrap();
        let state_boundaries = rebuilt
            .dependencies()
            .iter()
            .filter(|dependency| dependency.edge().kind() == DependencyKind::StateBoundary)
            .collect::<Vec<_>>();
        assert_eq!(state_boundaries.len(), 1);
        assert_eq!(
            assignment_node(&rebuilt, state_boundaries[0].source(), boundary.site()).unwrap(),
            original_order
        );
        assert_eq!(
            signal_name(&rebuilt, state_boundaries[0].target(), boundary.site()).unwrap(),
            "d0"
        );
        let boundary_assignment = rebuilt.assignment_id(boundary_order).unwrap();
        assert!(rebuilt.dependencies().iter().any(|dependency| {
            dependency.source() == boundary_assignment
                && dependency.edge().kind() == DependencyKind::Drive
                && signal_name(&rebuilt, dependency.target(), boundary.site()).unwrap() == "d1"
        }));

        let verified_paths = applied.verification.applied().paths();
        assert_eq!(verified_paths.len(), 1);
        assert_eq!(
            verified_paths[0].assignment_orders(),
            &[original_order, boundary_order, public_order]
        );
        assert!(
            applied
                .signal_metadata
                .iter()
                .find(|entry| entry.name() == "d0")
                .unwrap()
                .roles()
                .contains(&TimingSignalRole::ModeledRegister)
        );
        assert!(
            !applied
                .signal_metadata
                .iter()
                .find(|entry| entry.name() == "d1")
                .unwrap()
                .roles()
                .contains(&TimingSignalRole::ModeledRegister)
        );

        let erased = applied
            .erasure
            .erase(&applied.lowered, &applied.assignment_provenance)
            .unwrap();
        assert_eq!(erased.lowered, original);
        assert_eq!(erased.signal_metadata, metadata);
        assert_eq!(erased.assignment_provenance, provenance);
    }

    #[test]
    fn actual_verifier_rejects_a_value_edge_that_bypasses_the_boundary_delay() {
        let (mut applied, _, _, _, graph, decomposition) = applied_compound_state_transform();
        let public_order = applied
            .facts
            .placements
            .iter()
            .find(|placement| matches!(placement.site, PlacementSite::PublicOutputSplit { .. }))
            .unwrap()
            .assignment_order;
        let assignment = applied
            .lowered
            .cell
            .items
            .iter_mut()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some(assignment),
                CellItem::Blank | CellItem::Comment(_) => None,
            })
            .nth(public_order)
            .unwrap();
        assignment.expr = Expr::atom("d0");

        let error = verify_applied_timing_transform(
            &applied.lowered,
            &applied.signal_metadata,
            &applied.assignment_provenance,
            &graph,
            &decomposition,
            &applied.facts,
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::AppliedPathReconstructionMismatch { .. }
        ));
    }

    #[test]
    fn actual_verifier_rejects_incomplete_or_provenance_inconsistent_typed_maps() {
        let (applied, _, _, _, graph, decomposition) = applied_compound_state_transform();

        let mut incomplete = applied.facts.clone();
        incomplete
            .assignment_orders
            .remove(&AppliedAssignmentId::Original(1));
        let error = verify_applied_timing_transform(
            &applied.lowered,
            &applied.signal_metadata,
            &applied.assignment_provenance,
            &graph,
            &decomposition,
            &incomplete,
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::InconsistentAnalysis { .. }
        ));

        let mut inconsistent = applied.facts.clone();
        let original_order = inconsistent
            .assignment_orders
            .remove(&AppliedAssignmentId::Original(0))
            .unwrap();
        inconsistent
            .assignment_orders
            .insert(AppliedAssignmentId::TimingGenerated(99), original_order);
        let error = verify_applied_timing_transform(
            &applied.lowered,
            &applied.signal_metadata,
            &applied.assignment_provenance,
            &graph,
            &decomposition,
            &inconsistent,
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::AppliedAssignmentMapping { .. }
        ));
    }

    #[test]
    fn erasure_rejects_a_corrupted_transformed_equation() {
        let (applied, _, _) = applied_operand_transform();
        let mut corrupted = applied.lowered.clone();
        let assignment = corrupted
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "y" => Some(assignment),
                CellItem::Blank | CellItem::Comment(_) | CellItem::Assignment(_) => None,
            })
            .unwrap();
        assignment.expr = Expr::atom("0");

        let error = applied
            .erasure
            .erase(&corrupted, &applied.assignment_provenance)
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::ErasureMismatch { .. }
        ));
    }

    #[test]
    fn materialization_distinguishes_empty_and_literal_zero_and_canonicalizes_addition() {
        let literal_zero = DelayTerm::from_timing_expr(timing("0")).unwrap();
        let a = DelayTerm::from_timing_expr(timing("A")).unwrap();
        let b = DelayTerm::from_timing_expr(timing("B")).unwrap();
        let delay = PlacementDelay::Three {
            rise: Vec::new(),
            fall: vec![literal_zero],
            turn_off: vec![a, b],
        };
        let actual = materialize_delay(&delay, &span(1)).unwrap();
        let DelayTuple::Three {
            rise,
            fall,
            turn_off,
        } = actual
        else {
            panic!("expected a three-entry tuple");
        };
        assert_eq!(rise, timing("0"));
        assert_eq!(fall, timing("0"));
        assert_eq!(
            turn_off,
            TimingExpr::operation(
                crate::ir::TimingOperator::Add,
                vec![timing("A"), timing("B")]
            )
            .unwrap()
        );
        assert!(delay.component(0).unwrap().is_empty());
        assert_eq!(delay.component(1).unwrap().len(), 1);
    }
}
