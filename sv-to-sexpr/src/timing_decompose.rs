//! Exact, deterministic planning of timing terms onto functional assignments.
//!
//! This module is deliberately pure. It identifies physical placement sites
//! and proves an exact symbolic cover, but it does not mutate [`crate::ir::Cell`]
//! or select names for inserted assignments. The independent verifier rebuilds
//! every epoch-bounded functional path from the accepted Milestone 15 graph
//! instead of trusting the solver's coverage records.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{Duration, Instant};

use crate::diagnostic::Span;
use crate::ir::{Cell, CellItem, DelayTuple, Expr, TimingExpr, TimingOperator};
use crate::timing_graph::{
    AssignmentDelayOrigin, AssignmentProvenance, CutTimingGraph, DependencyKind, PublicOutputSplit,
    TimingAnalysisReport, TimingConstraint, TimingConstraintId, TimingControlId, TimingGraph,
    TimingNodeId, TimingNodeKind, TimingSense, TimingSignalRole,
};
use crate::timing_terms::{
    AdditiveDelay, AdditiveDelayTuple, AdditiveDelayTupleContribution, DelayTerm, TermRange,
    TimingTermsError,
};

/// A durable physical location at which a delay can be represented.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlacementSite {
    /// The result of an existing emitted assignment, before its target signal.
    ExistingAssignment {
        node: TimingNodeId,
        assignment_order: usize,
    },
    /// One exact dependency occurrence in source/build order.
    DependencyEdge {
        dependency_order: usize,
        source: TimingNodeId,
        target: TimingNodeId,
    },
    /// A typed raw/public split after an output's internal value region.
    PublicOutputSplit { signal: TimingNodeId },
}

/// The actual local delay value carried by one physical placement.
///
/// Empty component vectors mean that the placement contributes no term to
/// that transition. They remain distinct from a vector containing the literal
/// timing term `0`; Phase 3 is responsible for IR-boundary materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementDelay {
    One(Vec<DelayTerm>),
    Two {
        rise: Vec<DelayTerm>,
        fall: Vec<DelayTerm>,
    },
    Three {
        rise: Vec<DelayTerm>,
        fall: Vec<DelayTerm>,
        turn_off: Vec<DelayTerm>,
    },
}

impl PlacementDelay {
    pub const fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Two { .. } => 2,
            Self::Three { .. } => 3,
        }
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn component(&self, index: usize) -> Option<&[DelayTerm]> {
        match (self, index) {
            (Self::One(value), 0) => Some(value),
            (Self::Two { rise, .. }, 0) | (Self::Three { rise, .. }, 0) => Some(rise),
            (Self::Two { fall, .. }, 1) | (Self::Three { fall, .. }, 1) => Some(fall),
            (Self::Three { turn_off, .. }, 2) => Some(turn_off),
            _ => None,
        }
    }

    pub fn components(&self) -> PlacementDelayComponents<'_> {
        PlacementDelayComponents {
            delay: self,
            index: 0,
        }
    }

    pub fn is_zero_contribution(&self) -> bool {
        self.components().all(<[DelayTerm]>::is_empty)
    }

    pub fn canonical_component(
        &self,
        index: usize,
    ) -> Result<Option<TimingExpr>, TimingTermsError> {
        let Some(terms) = self.component(index) else {
            return Ok(None);
        };
        match terms {
            [] => Ok(None),
            [term] => Ok(Some(term.as_timing_expr().clone())),
            _ => TimingExpr::operation(
                TimingOperator::Add,
                terms
                    .iter()
                    .map(|term| term.as_timing_expr().clone())
                    .collect(),
            )
            .map(Some)
            .map_err(TimingTermsError::InvalidTimingExpression),
        }
    }

    fn from_component_terms(
        tuple: &AdditiveDelayTuple,
        ranges: &[TermRange],
    ) -> Result<Self, TimingTermsError> {
        if tuple.len() != ranges.len() {
            return Err(TimingTermsError::TupleArityMismatch {
                expected: tuple.len(),
                actual: ranges.len(),
            });
        }
        let selected = |index: usize| {
            let range = ranges[index];
            let component = tuple
                .component(index)
                .expect("range arity was checked against tuple arity");
            component
                .select_range(range)
                .map(|selection| selection.terms().to_vec())
        };
        match tuple {
            AdditiveDelayTuple::One(_) => Ok(Self::One(selected(0)?)),
            AdditiveDelayTuple::Two { .. } => Ok(Self::Two {
                rise: selected(0)?,
                fall: selected(1)?,
            }),
            AdditiveDelayTuple::Three { .. } => Ok(Self::Three {
                rise: selected(0)?,
                fall: selected(1)?,
                turn_off: selected(2)?,
            }),
        }
    }

    fn oriented(&self, orientation: PathOrientation) -> Option<Self> {
        match (self, orientation) {
            (_, PathOrientation::Positive) | (Self::One(_), PathOrientation::Ambiguous) => {
                Some(self.clone())
            }
            (Self::One(value), PathOrientation::Negative) => Some(Self::One(value.clone())),
            (Self::Two { rise, fall }, PathOrientation::Negative) => Some(Self::Two {
                rise: fall.clone(),
                fall: rise.clone(),
            }),
            (
                Self::Three {
                    rise,
                    fall,
                    turn_off,
                },
                PathOrientation::Negative,
            ) => Some(Self::Three {
                rise: fall.clone(),
                fall: rise.clone(),
                turn_off: turn_off.clone(),
            }),
            (Self::Two { rise, fall }, PathOrientation::Ambiguous) if rise == fall => {
                Some(self.clone())
            }
            (
                Self::Three {
                    rise,
                    fall,
                    turn_off: _,
                },
                PathOrientation::Ambiguous,
            ) if rise == fall => Some(self.clone()),
            (Self::Two { .. } | Self::Three { .. }, PathOrientation::Ambiguous) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlacementDelayComponents<'a> {
    delay: &'a PlacementDelay,
    index: usize,
}

impl<'a> Iterator for PlacementDelayComponents<'a> {
    type Item = &'a [DelayTerm];

    fn next(&mut self) -> Option<Self::Item> {
        let component = self.delay.component(self.index)?;
        self.index += 1;
        Some(component)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.delay.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for PlacementDelayComponents<'_> {}
impl std::iter::FusedIterator for PlacementDelayComponents<'_> {}

/// Source-specific positional coverage produced by one physical placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementCoverage {
    path_id: DecompositionPathId,
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    contribution: AdditiveDelayTupleContribution,
}

impl PlacementCoverage {
    pub const fn path_id(&self) -> DecompositionPathId {
        self.path_id
    }

    pub const fn constraint_id(&self) -> TimingConstraintId {
        self.constraint_id
    }

    pub const fn control_id(&self) -> TimingControlId {
        self.control_id
    }

    pub fn contribution(&self) -> &AdditiveDelayTupleContribution {
        &self.contribution
    }
}

/// One selected physical delay and all source paths which it affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayPlacement {
    site: PlacementSite,
    delay: PlacementDelay,
    coverage: Vec<PlacementCoverage>,
}

impl DelayPlacement {
    pub fn site(&self) -> &PlacementSite {
        &self.site
    }

    pub fn delay(&self) -> &PlacementDelay {
        &self.delay
    }

    pub fn coverage(&self) -> &[PlacementCoverage] {
        &self.coverage
    }

    #[cfg(test)]
    pub(crate) fn test_only(site: PlacementSite, delay: PlacementDelay) -> Self {
        Self {
            site,
            delay,
            coverage: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecompositionPathId(usize);

impl DecompositionPathId {
    pub const fn ordinal(self) -> usize {
        self.0
    }
}

impl fmt::Display for DecompositionPathId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "f{}", self.0)
    }
}

/// Deterministic identity of one epoch-bounded functional path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompositionPath {
    id: DecompositionPathId,
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    nodes: Vec<TimingNodeId>,
    dependency_orders: Vec<usize>,
    sites: Vec<PlacementSite>,
}

impl DecompositionPath {
    pub const fn id(&self) -> DecompositionPathId {
        self.id
    }

    pub const fn constraint_id(&self) -> TimingConstraintId {
        self.constraint_id
    }

    pub const fn control_id(&self) -> TimingControlId {
        self.control_id
    }

    pub fn nodes(&self) -> &[TimingNodeId] {
        &self.nodes
    }

    pub fn dependency_orders(&self) -> &[usize] {
        &self.dependency_orders
    }

    pub fn sites(&self) -> &[PlacementSite] {
        &self.sites
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedDelayComponent {
    All,
    Rise,
    Fall,
    TurnOff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPath {
    path_id: DecompositionPathId,
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    components: Vec<VerifiedDelayComponent>,
}

impl VerifiedPath {
    pub const fn path_id(&self) -> DecompositionPathId {
        self.path_id
    }

    pub const fn constraint_id(&self) -> TimingConstraintId {
        self.constraint_id
    }

    pub const fn control_id(&self) -> TimingControlId {
        self.control_id
    }

    pub fn components(&self) -> &[VerifiedDelayComponent] {
        &self.components
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecompositionVerification {
    paths: Vec<VerifiedPath>,
}

impl DecompositionVerification {
    pub fn paths(&self) -> &[VerifiedPath] {
        &self.paths
    }
}

/// A complete pure placement plan and its independently reconstructed proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decomposition {
    paths: Vec<DecompositionPath>,
    placements: Vec<DelayPlacement>,
    verification: DecompositionVerification,
}

impl Decomposition {
    pub fn paths(&self) -> &[DecompositionPath] {
        &self.paths
    }

    pub fn placements(&self) -> &[DelayPlacement] {
        &self.placements
    }

    pub fn verification(&self) -> &DecompositionVerification {
        &self.verification
    }

    #[cfg(test)]
    pub(crate) fn test_only(placements: Vec<DelayPlacement>) -> Self {
        Self {
            paths: Vec::new(),
            placements,
            verification: DecompositionVerification::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_only_replacing_placements(&self, placements: Vec<DelayPlacement>) -> Self {
        Self {
            paths: self.paths.clone(),
            placements,
            verification: self.verification.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedVerificationMap {
    original_to_transformed: BTreeMap<usize, usize>,
    generated_assignments: BTreeSet<usize>,
    empty_components: BTreeMap<usize, Vec<bool>>,
}

impl AppliedVerificationMap {
    pub fn new(
        original_to_transformed: BTreeMap<usize, usize>,
        generated_assignments: BTreeSet<usize>,
        empty_components: BTreeMap<usize, Vec<bool>>,
    ) -> Self {
        Self {
            original_to_transformed,
            generated_assignments,
            empty_components,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedVerifiedPath {
    path_id: DecompositionPathId,
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    assignment_orders: Vec<usize>,
}

impl AppliedVerifiedPath {
    pub const fn path_id(&self) -> DecompositionPathId {
        self.path_id
    }

    pub const fn constraint_id(&self) -> TimingConstraintId {
        self.constraint_id
    }

    pub const fn control_id(&self) -> TimingControlId {
        self.control_id
    }

    pub fn assignment_orders(&self) -> &[usize] {
        &self.assignment_orders
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppliedModelVerification {
    paths: Vec<AppliedVerifiedPath>,
}

impl AppliedModelVerification {
    pub fn paths(&self) -> &[AppliedVerifiedPath] {
        &self.paths
    }
}

/// One rebuilt transformed representation supplied to the independent
/// verifier as a coherent snapshot.
pub struct AppliedModelSnapshot<'a> {
    graph: &'a TimingGraph,
    cut_graph: &'a CutTimingGraph,
    report: &'a TimingAnalysisReport,
    cell: &'a Cell,
    assignment_provenance: &'a [AssignmentProvenance],
}

impl<'a> AppliedModelSnapshot<'a> {
    pub fn new(
        graph: &'a TimingGraph,
        cut_graph: &'a CutTimingGraph,
        report: &'a TimingAnalysisReport,
        cell: &'a Cell,
        assignment_provenance: &'a [AssignmentProvenance],
    ) -> Self {
        Self {
            graph,
            cut_graph,
            report,
            cell,
            assignment_provenance,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecompositionErrorKind {
    InconsistentAnalysis {
        detail: String,
    },
    MissingFunctionalPath {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
    },
    AmbiguousBoundaryTraversal {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        dependency_order: usize,
    },
    CandidateSpaceTooLarge {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        count: usize,
    },
    UnrepresentableSense {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        path_id: DecompositionPathId,
        component: usize,
        site: PlacementSite,
    },
    IncompatibleSiteValues {
        site: PlacementSite,
        first_constraint_id: TimingConstraintId,
        first_control_id: TimingControlId,
        conflicting_constraint_id: TimingConstraintId,
        conflicting_control_id: TimingControlId,
    },
    NoExactCover {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        component: usize,
        term_position: usize,
    },
    DuplicatePlacementSite {
        site: PlacementSite,
    },
    StalePlacementSite {
        site: PlacementSite,
    },
    PlacementTupleArity {
        site: PlacementSite,
        expected: usize,
        actual: usize,
    },
    ReconstructionMismatch {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        component: usize,
        term_position: usize,
        site: PlacementSite,
    },
    UncoveredTerm {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        component: usize,
        term_position: usize,
    },
    CoverageMismatch {
        site: PlacementSite,
    },
    SymbolicTerms {
        detail: String,
    },
    ReservedDelayName {
        name: String,
    },
    PlacementConflict {
        site: PlacementSite,
        detail: String,
    },
    UnsupportedPlacement {
        site: PlacementSite,
        detail: String,
    },
    PublicSplitInout {
        signal: String,
    },
    PublicSplitDriverCount {
        signal: String,
        drivers: usize,
    },
    AppliedDelayMismatch {
        site: PlacementSite,
        detail: String,
    },
    AppliedPathIdentityMismatch {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        detail: String,
    },
    AppliedPathReconstructionMismatch {
        constraint_id: TimingConstraintId,
        control_id: TimingControlId,
        path_id: DecompositionPathId,
        component: usize,
        detail: String,
    },
    AppliedAssignmentMapping {
        assignment_order: usize,
        detail: String,
    },
    UnmappedIntrinsicDelay {
        assignment_order: usize,
    },
    ErasureMismatch {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompositionError {
    span: Span,
    kind: DecompositionErrorKind,
}

impl DecompositionError {
    pub(crate) fn new(span: Span, kind: DecompositionErrorKind) -> Self {
        Self { span, kind }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn kind(&self) -> &DecompositionErrorKind {
        &self.kind
    }
}

impl fmt::Display for DecompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: timing decomposition error: ",
            self.span.path.display(),
            self.span.line,
            self.span.column
        )?;
        match &self.kind {
            DecompositionErrorKind::InconsistentAnalysis { detail } => {
                write!(formatter, "inconsistent timing analysis: {detail}")
            }
            DecompositionErrorKind::MissingFunctionalPath {
                constraint_id,
                control_id,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} has no epoch-bounded functional path"
            ),
            DecompositionErrorKind::AmbiguousBoundaryTraversal {
                constraint_id,
                control_id,
                dependency_order,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} requires ambiguous second boundary dependency {dependency_order}"
            ),
            DecompositionErrorKind::CandidateSpaceTooLarge {
                constraint_id,
                control_id,
                count,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} produced {count} exact placement candidates"
            ),
            DecompositionErrorKind::UnrepresentableSense {
                constraint_id,
                control_id,
                path_id,
                component,
                site,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} path {path_id} component {component} has transition-ambiguous placement {site:?}"
            ),
            DecompositionErrorKind::IncompatibleSiteValues {
                site,
                first_constraint_id,
                first_control_id,
                conflicting_constraint_id,
                conflicting_control_id,
            } => write!(
                formatter,
                "placement {site:?} requires incompatible values for constraint {first_constraint_id} control {first_control_id} and constraint {conflicting_constraint_id} control {conflicting_control_id}"
            ),
            DecompositionErrorKind::NoExactCover {
                constraint_id,
                control_id,
                component,
                term_position,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} component {component} term {term_position} has no exact physical cover"
            ),
            DecompositionErrorKind::DuplicatePlacementSite { site } => {
                write!(
                    formatter,
                    "physical placement site {site:?} appears more than once"
                )
            }
            DecompositionErrorKind::StalePlacementSite { site } => {
                write!(formatter, "placement refers to stale graph site {site:?}")
            }
            DecompositionErrorKind::PlacementTupleArity {
                site,
                expected,
                actual,
            } => write!(
                formatter,
                "placement {site:?} has tuple arity {actual}, expected {expected}"
            ),
            DecompositionErrorKind::ReconstructionMismatch {
                constraint_id,
                control_id,
                component,
                term_position,
                site,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} component {component} first mismatches at term {term_position} on {site:?}"
            ),
            DecompositionErrorKind::UncoveredTerm {
                constraint_id,
                control_id,
                component,
                term_position,
            } => write!(
                formatter,
                "constraint {constraint_id} control {control_id} component {component} remains uncovered at term {term_position}"
            ),
            DecompositionErrorKind::CoverageMismatch { site } => {
                write!(formatter, "solver coverage metadata disagrees at {site:?}")
            }
            DecompositionErrorKind::SymbolicTerms { detail } => {
                write!(formatter, "exact symbolic term operation failed: {detail}")
            }
            DecompositionErrorKind::ReservedDelayName { name } => {
                write!(
                    formatter,
                    "deterministic timing name `{name}` is already reserved"
                )
            }
            DecompositionErrorKind::PlacementConflict { site, detail } => {
                write!(
                    formatter,
                    "placement {site:?} conflicts with the lowered cell: {detail}"
                )
            }
            DecompositionErrorKind::UnsupportedPlacement { site, detail } => {
                write!(
                    formatter,
                    "placement {site:?} cannot be materialized: {detail}"
                )
            }
            DecompositionErrorKind::PublicSplitInout { signal } => {
                write!(
                    formatter,
                    "public output split cannot rewrite inout `{signal}`"
                )
            }
            DecompositionErrorKind::PublicSplitDriverCount { signal, drivers } => write!(
                formatter,
                "public output split requires one driver for `{signal}`, found {drivers}"
            ),
            DecompositionErrorKind::AppliedDelayMismatch { site, detail } => {
                write!(
                    formatter,
                    "applied placement {site:?} is not exact: {detail}"
                )
            }
            DecompositionErrorKind::AppliedPathIdentityMismatch {
                constraint_id,
                control_id,
                detail,
            } => write!(
                formatter,
                "applied constraint {constraint_id} control {control_id} path identity differs: {detail}"
            ),
            DecompositionErrorKind::AppliedPathReconstructionMismatch {
                constraint_id,
                control_id,
                path_id,
                component,
                detail,
            } => write!(
                formatter,
                "applied constraint {constraint_id} control {control_id} path {path_id} component {component} does not reconstruct: {detail}"
            ),
            DecompositionErrorKind::AppliedAssignmentMapping {
                assignment_order,
                detail,
            } => write!(
                formatter,
                "applied assignment {assignment_order} mapping is invalid: {detail}"
            ),
            DecompositionErrorKind::UnmappedIntrinsicDelay { assignment_order } => write!(
                formatter,
                "constrained assignment {assignment_order} retains an unmapped intrinsic delay"
            ),
            DecompositionErrorKind::ErasureMismatch { detail } => {
                write!(
                    formatter,
                    "timing erasure rejected transformed state: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for DecompositionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathOrientation {
    Positive,
    Negative,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSite {
    site: PlacementSite,
    orientation_to_target: PathOrientation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionalPath {
    public: DecompositionPath,
    sites: Vec<PathSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateEffect {
    path_index: usize,
    contribution: AdditiveDelayTupleContribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    site: PlacementSite,
    delay: PlacementDelay,
    effects: Vec<CandidateEffect>,
    preference: CandidatePreference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CandidatePreference {
    kind: u8,
    non_shared: bool,
    target_distance: usize,
    constraint_order: u32,
    control_order: u32,
    path_order: usize,
    site_order: usize,
}

/// Produces an exact pure placement plan from the accepted Milestone 15 model.
pub fn decompose_timing(
    graph: &TimingGraph,
    cut_graph: &CutTimingGraph,
    report: &TimingAnalysisReport,
) -> Result<Decomposition, DecompositionError> {
    validate_analysis_inputs(graph, cut_graph, report)?;
    let paths = enumerate_functional_paths(graph, report)?;
    let (candidates, blockers) = build_candidates(graph, report, &paths)?;
    let selected = match solve_exact_cover(graph, &paths, &candidates) {
        Ok(selected) => selected,
        Err(_) if !blockers.is_empty() => return Err(earliest_blocker(blockers)),
        Err(error) => return Err(error),
    };
    let placements = selected
        .iter()
        .map(|&index| {
            let candidate = &candidates[index];
            DelayPlacement {
                site: candidate.site.clone(),
                delay: candidate.delay.clone(),
                coverage: candidate
                    .effects
                    .iter()
                    .map(|effect| {
                        let path = &paths[effect.path_index].public;
                        PlacementCoverage {
                            path_id: path.id,
                            constraint_id: path.constraint_id,
                            control_id: path.control_id,
                            contribution: effect.contribution.clone(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    let mut decomposition = Decomposition {
        paths: paths.iter().map(|path| path.public.clone()).collect(),
        placements,
        verification: DecompositionVerification::default(),
    };
    decomposition.verification = verify_decomposition(graph, cut_graph, report, &decomposition)?;
    Ok(decomposition)
}

/// Independently reconstructs all paths and every tuple component from actual
/// physical placement values.
pub fn verify_decomposition(
    graph: &TimingGraph,
    cut_graph: &CutTimingGraph,
    report: &TimingAnalysisReport,
    decomposition: &Decomposition,
) -> Result<DecompositionVerification, DecompositionError> {
    validate_analysis_inputs(graph, cut_graph, report)?;
    let paths = enumerate_functional_paths(graph, report)?;
    let public_paths = paths
        .iter()
        .map(|path| path.public.clone())
        .collect::<Vec<_>>();
    if public_paths != decomposition.paths {
        return Err(analysis_error(
            graph,
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "decomposition path identities are stale".to_string(),
            },
        ));
    }

    validate_placement_sites(graph, report, decomposition)?;
    let mut expected_coverage =
        vec![Vec::<PlacementCoverage>::new(); decomposition.placements.len()];
    let mut used_placements = vec![false; decomposition.placements.len()];
    let mut verified_paths = Vec::with_capacity(paths.len());

    for path in &paths {
        let constraint = constraint_for_path(graph, path)?;
        let mut placements_on_path = path
            .sites
            .iter()
            .enumerate()
            .filter_map(|(site_order, path_site)| {
                decomposition
                    .placements
                    .iter()
                    .enumerate()
                    .find(|(_, placement)| placement.site == path_site.site)
                    .map(|(placement_index, placement)| {
                        (site_order, path_site, placement_index, placement)
                    })
            })
            .collect::<Vec<_>>();
        placements_on_path.sort_by_key(|(site_order, _, _, _)| *site_order);

        let mut contributions = Vec::with_capacity(placements_on_path.len());
        let mut cursors = vec![0_usize; constraint.additive_delay().len()];
        for (_, path_site, placement_index, placement) in placements_on_path {
            used_placements[placement_index] = true;
            if placement.delay.len() != constraint.additive_delay().len() {
                return Err(control_error(
                    graph,
                    path,
                    DecompositionErrorKind::PlacementTupleArity {
                        site: placement.site.clone(),
                        expected: constraint.additive_delay().len(),
                        actual: placement.delay.len(),
                    },
                ));
            }
            let target_delay = placement
                .delay
                .oriented(path_site.orientation_to_target)
                .ok_or_else(|| {
                    control_error(
                        graph,
                        path,
                        DecompositionErrorKind::UnrepresentableSense {
                            constraint_id: path.public.constraint_id,
                            control_id: path.public.control_id,
                            path_id: path.public.id,
                            component: 0,
                            site: placement.site.clone(),
                        },
                    )
                })?;
            let mut ranges = Vec::with_capacity(target_delay.len());
            for (component, cursor) in cursors.iter_mut().enumerate() {
                let terms = target_delay
                    .component(component)
                    .expect("component is bounded by placement arity");
                let source = constraint
                    .additive_delay()
                    .component(component)
                    .expect("component is bounded by constraint arity");
                let start = *cursor;
                let end = start.saturating_add(terms.len());
                if end > source.len() || source.terms()[start..end] != *terms {
                    return Err(control_error(
                        graph,
                        path,
                        DecompositionErrorKind::ReconstructionMismatch {
                            constraint_id: path.public.constraint_id,
                            control_id: path.public.control_id,
                            component,
                            term_position: start,
                            site: placement.site.clone(),
                        },
                    ));
                }
                ranges.push(
                    TermRange::new(start, end)
                        .map_err(|error| symbolic_error(constraint.span().clone(), error))?,
                );
                *cursor = end;
            }
            let contribution = constraint
                .additive_delay()
                .select_ranges(&ranges)
                .map_err(|error| symbolic_error(constraint.span().clone(), error))?;
            expected_coverage[placement_index].push(PlacementCoverage {
                path_id: path.public.id,
                constraint_id: path.public.constraint_id,
                control_id: path.public.control_id,
                contribution: contribution.clone(),
            });
            contributions.push(contribution);
        }

        for (component, (&cursor, source)) in cursors
            .iter()
            .zip(constraint.additive_delay().components())
            .enumerate()
        {
            if cursor != source.len() {
                return Err(control_error(
                    graph,
                    path,
                    DecompositionErrorKind::UncoveredTerm {
                        constraint_id: path.public.constraint_id,
                        control_id: path.public.control_id,
                        component,
                        term_position: cursor,
                    },
                ));
            }
        }
        let rebuilt = constraint
            .additive_delay()
            .recompose_contributions(&contributions)
            .map_err(|error| symbolic_error(constraint.span().clone(), error))?;
        if &rebuilt != constraint.delay() {
            return Err(control_error(
                graph,
                path,
                DecompositionErrorKind::InconsistentAnalysis {
                    detail: "exact term cover did not recover the retained source tuple"
                        .to_string(),
                },
            ));
        }
        verified_paths.push(VerifiedPath {
            path_id: path.public.id,
            constraint_id: path.public.constraint_id,
            control_id: path.public.control_id,
            components: verified_components(constraint.delay()),
        });
    }

    for (index, placement) in decomposition.placements.iter().enumerate() {
        if !used_placements[index] {
            return Err(site_error(
                graph,
                report,
                &placement.site,
                DecompositionErrorKind::StalePlacementSite {
                    site: placement.site.clone(),
                },
            ));
        }
        if placement.coverage != expected_coverage[index] {
            return Err(site_error(
                graph,
                report,
                &placement.site,
                DecompositionErrorKind::CoverageMismatch {
                    site: placement.site.clone(),
                },
            ));
        }
    }

    Ok(DecompositionVerification {
        paths: verified_paths,
    })
}

/// Independently verifies the transformed model from its actual graph and
/// assignment delay tuples.
///
/// The symbolic placement values and coverage records are not consulted.
/// `mapping` is used only to project the transformed graph back to durable
/// baseline assignment identities and to preserve the IR-only distinction
/// between an absent tuple contribution and a literal timing term `0`.
pub fn verify_applied_model(
    original_graph: &TimingGraph,
    decomposition: &Decomposition,
    model: &AppliedModelSnapshot<'_>,
    mapping: &AppliedVerificationMap,
) -> Result<AppliedModelVerification, DecompositionError> {
    let AppliedModelSnapshot {
        graph,
        cut_graph,
        report,
        cell,
        assignment_provenance,
    } = model;
    validate_analysis_inputs(graph, cut_graph, report)?;
    if original_graph.constraints() != graph.constraints() {
        return Err(analysis_error(
            graph,
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "applied graph did not retain the exact original constraints".to_string(),
            },
        ));
    }

    let assignments = cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    if assignments.len() != assignment_provenance.len() {
        return Err(analysis_error(
            graph,
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "applied assignments and provenance are not aligned".to_string(),
            },
        ));
    }

    let expected_original_assignments = original_graph
        .nodes()
        .filter_map(|node| match node.kind() {
            TimingNodeKind::Assignment(assignment) => Some(assignment.assignment_order()),
            TimingNodeKind::Signal(_) => None,
        })
        .collect::<BTreeSet<_>>();
    if mapping
        .original_to_transformed
        .keys()
        .copied()
        .collect::<BTreeSet<_>>()
        != expected_original_assignments
    {
        return Err(analysis_error(
            graph,
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "original assignment projection is not total for the baseline graph"
                    .to_string(),
            },
        ));
    }

    let mut transformed_to_original = BTreeMap::new();
    for (&original, &transformed) in &mapping.original_to_transformed {
        if transformed >= assignments.len()
            || transformed_to_original
                .insert(transformed, original)
                .is_some()
        {
            return Err(DecompositionError::new(
                assignment_provenance
                    .get(transformed)
                    .map_or_else(|| analysis_span(graph), |value| value.span().clone()),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: transformed,
                    detail:
                        "original assignment projection is missing, duplicated, or out of bounds"
                            .to_string(),
                },
            ));
        }
    }
    for &generated in &mapping.generated_assignments {
        if generated >= assignments.len() || transformed_to_original.contains_key(&generated) {
            return Err(DecompositionError::new(
                assignment_provenance
                    .get(generated)
                    .map_or_else(|| analysis_span(graph), |value| value.span().clone()),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: generated,
                    detail: "generated assignment projection is out of bounds or aliases an original assignment"
                        .to_string(),
                },
            ));
        }
    }
    if transformed_to_original.len() + mapping.generated_assignments.len() != assignments.len() {
        return Err(analysis_error(
            graph,
            DecompositionErrorKind::InconsistentAnalysis {
                detail: "applied assignment projection does not cover the transformed cell"
                    .to_string(),
            },
        ));
    }
    for (&order, empty) in &mapping.empty_components {
        let Some((assignment, provenance)) =
            assignments.get(order).zip(assignment_provenance.get(order))
        else {
            return Err(analysis_error(
                graph,
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: order,
                    detail: "empty-component metadata refers to an absent assignment".to_string(),
                },
            ));
        };
        if empty.len() != assignment.delay.len()
            || !(provenance.delay_origin() == AssignmentDelayOrigin::DecompositionPlacement
                || provenance.delay_origin().is_intrinsic_source_delay())
        {
            return Err(DecompositionError::new(
                provenance.span().clone(),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: order,
                    detail:
                        "empty-component metadata is inconsistent with tuple arity or delay origin"
                            .to_string(),
                },
            ));
        }
    }
    for (order, provenance) in assignment_provenance.iter().enumerate() {
        if provenance.delay_origin() == AssignmentDelayOrigin::DecompositionPlacement
            && !mapping.empty_components.contains_key(&order)
        {
            return Err(DecompositionError::new(
                provenance.span().clone(),
                DecompositionErrorKind::AppliedAssignmentMapping {
                    assignment_order: order,
                    detail: "a decomposition delay lacks empty-component metadata".to_string(),
                },
            ));
        }
    }

    let paths = enumerate_functional_paths(graph, report)?;
    verify_applied_path_identities(
        original_graph,
        decomposition,
        graph,
        &paths,
        &transformed_to_original,
        &mapping.generated_assignments,
    )?;

    let mut used_applied_delays = BTreeSet::new();
    let mut verified_paths = Vec::with_capacity(paths.len());
    for path in &paths {
        let constraint = constraint_for_path(graph, path)?;
        let mut reconstructed = vec![Vec::<DelayTerm>::new(); constraint.delay().len()];
        let mut path_assignment_orders = Vec::new();

        for path_site in &path.sites {
            let PlacementSite::ExistingAssignment {
                assignment_order, ..
            } = path_site.site
            else {
                continue;
            };
            if path_assignment_orders.last() == Some(&assignment_order) {
                continue;
            }
            path_assignment_orders.push(assignment_order);
            let assignment = assignments[assignment_order];
            let provenance = &assignment_provenance[assignment_order];
            let empty_components = mapping.empty_components.get(&assignment_order);
            let Some(local_delay) = actual_assignment_delay(
                assignment_order,
                &assignment.delay,
                provenance,
                empty_components,
            )?
            else {
                continue;
            };
            if provenance.delay_origin().is_intrinsic_source_delay() && empty_components.is_none() {
                return Err(DecompositionError::new(
                    provenance.span().clone(),
                    DecompositionErrorKind::UnmappedIntrinsicDelay { assignment_order },
                ));
            }
            if empty_components.is_some() {
                used_applied_delays.insert(assignment_order);
            }
            let target_delay = local_delay
                .oriented(path_site.orientation_to_target)
                .ok_or_else(|| {
                    DecompositionError::new(
                        provenance.span().clone(),
                        DecompositionErrorKind::AppliedPathReconstructionMismatch {
                            constraint_id: path.public.constraint_id,
                            control_id: path.public.control_id,
                            path_id: path.public.id,
                            component: 0,
                            detail: format!(
                                "assignment {assignment_order} has distinct transition components after a non-unate/conditional suffix"
                            ),
                        },
                    )
                })?;
            for (component, terms) in target_delay.components().enumerate() {
                reconstructed[component].extend_from_slice(terms);
            }
        }

        for (component, (actual, expected)) in reconstructed
            .iter()
            .zip(constraint.additive_delay().components())
            .enumerate()
        {
            if actual != expected.terms() {
                return Err(control_error(
                    graph,
                    path,
                    DecompositionErrorKind::AppliedPathReconstructionMismatch {
                        constraint_id: path.public.constraint_id,
                        control_id: path.public.control_id,
                        path_id: path.public.id,
                        component,
                        detail: format!(
                            "actual terms={actual:?}, expected terms={:?}",
                            expected.terms()
                        ),
                    },
                ));
            }
        }
        verified_paths.push(AppliedVerifiedPath {
            path_id: path.public.id,
            constraint_id: path.public.constraint_id,
            control_id: path.public.control_id,
            assignment_orders: path_assignment_orders,
        });
    }

    if let Some(&unused) = mapping
        .empty_components
        .keys()
        .find(|order| !used_applied_delays.contains(order))
    {
        return Err(DecompositionError::new(
            assignment_provenance[unused].span().clone(),
            DecompositionErrorKind::AppliedAssignmentMapping {
                assignment_order: unused,
                detail: "applied delay assignment is not on any retained timing path".to_string(),
            },
        ));
    }

    Ok(AppliedModelVerification {
        paths: verified_paths,
    })
}

fn verify_applied_path_identities(
    original_graph: &TimingGraph,
    decomposition: &Decomposition,
    graph: &TimingGraph,
    paths: &[FunctionalPath],
    transformed_to_original: &BTreeMap<usize, usize>,
    generated_assignments: &BTreeSet<usize>,
) -> Result<(), DecompositionError> {
    type PathKey = (TimingConstraintId, TimingControlId);
    let mut expected = BTreeMap::<PathKey, Vec<Vec<usize>>>::new();
    for path in &decomposition.paths {
        let signature = path
            .nodes
            .iter()
            .filter_map(|node| match original_graph.node(*node)?.kind() {
                TimingNodeKind::Assignment(assignment) => Some(assignment.assignment_order()),
                TimingNodeKind::Signal(_) => None,
            })
            .collect::<Vec<_>>();
        expected
            .entry((path.constraint_id, path.control_id))
            .or_default()
            .push(signature);
    }
    let mut actual = BTreeMap::<PathKey, Vec<Vec<usize>>>::new();
    for path in paths {
        let mut signature = Vec::new();
        for node in &path.public.nodes {
            let Some(TimingNodeKind::Assignment(assignment)) =
                graph.node(*node).map(|node| node.kind())
            else {
                continue;
            };
            let order = assignment.assignment_order();
            if let Some(&original) = transformed_to_original.get(&order) {
                signature.push(original);
            } else if !generated_assignments.contains(&order) {
                return Err(DecompositionError::new(
                    graph
                        .node(*node)
                        .map_or_else(|| analysis_span(graph), |node| node.span().clone()),
                    DecompositionErrorKind::AppliedAssignmentMapping {
                        assignment_order: order,
                        detail: "path contains an assignment with no typed projection".to_string(),
                    },
                ));
            }
        }
        actual
            .entry((path.public.constraint_id, path.public.control_id))
            .or_default()
            .push(signature);
    }
    for signatures in expected.values_mut().chain(actual.values_mut()) {
        signatures.sort();
    }
    if expected == actual {
        return Ok(());
    }
    let key = expected
        .keys()
        .chain(actual.keys())
        .copied()
        .find(|key| expected.get(key) != actual.get(key))
        .unwrap_or_else(|| {
            let constraint = graph
                .constraints()
                .first()
                .expect("differing non-empty path maps have a constraint");
            (constraint.id(), constraint.controls()[0].id())
        });
    let span = graph
        .constraints()
        .get(key.0.ordinal() as usize)
        .and_then(|constraint| {
            constraint
                .controls()
                .iter()
                .find(|control| control.id() == key.1)
                .map(|control| control.source().span().clone())
        })
        .unwrap_or_else(|| analysis_span(graph));
    Err(DecompositionError::new(
        span,
        DecompositionErrorKind::AppliedPathIdentityMismatch {
            constraint_id: key.0,
            control_id: key.1,
            detail: format!(
                "actual projected paths={:?}, expected paths={:?}",
                actual.get(&key),
                expected.get(&key)
            ),
        },
    ))
}

fn actual_assignment_delay(
    assignment_order: usize,
    delay: &DelayTuple,
    provenance: &AssignmentProvenance,
    empty_components: Option<&Vec<bool>>,
) -> Result<Option<PlacementDelay>, DecompositionError> {
    let zero_origin = matches!(
        provenance.delay_origin(),
        AssignmentDelayOrigin::ImplicitZero
            | AssignmentDelayOrigin::GeneratedLogicalTemporaryZero
            | AssignmentDelayOrigin::KeeperZero
    );
    if zero_origin {
        if delay.components().all(is_zero_timing_atom) {
            return Ok(None);
        }
        return Err(DecompositionError::new(
            provenance.span().clone(),
            DecompositionErrorKind::AppliedAssignmentMapping {
                assignment_order,
                detail: format!(
                    "delay origin {:?} carries a nonzero actual tuple",
                    provenance.delay_origin()
                ),
            },
        ));
    }
    if provenance.delay_origin() == AssignmentDelayOrigin::LegacySelectedSpecifyFallback {
        return Err(DecompositionError::new(
            provenance.span().clone(),
            DecompositionErrorKind::AppliedAssignmentMapping {
                assignment_order,
                detail: "legacy first-path fallback is forbidden in an applied model".to_string(),
            },
        ));
    }

    let empty = empty_components
        .cloned()
        .unwrap_or_else(|| vec![false; delay.len()]);
    if empty.len() != delay.len() {
        return Err(DecompositionError::new(
            provenance.span().clone(),
            DecompositionErrorKind::AppliedAssignmentMapping {
                assignment_order,
                detail: format!(
                    "empty-component arity {} differs from actual tuple arity {}",
                    empty.len(),
                    delay.len()
                ),
            },
        ));
    }
    let mut components = Vec::with_capacity(delay.len());
    for (index, expression) in delay.components().enumerate() {
        if empty[index] {
            if !is_zero_timing_atom(expression) {
                return Err(DecompositionError::new(
                    provenance.span().clone(),
                    DecompositionErrorKind::AppliedAssignmentMapping {
                        assignment_order,
                        detail: format!(
                            "empty component {index} is not materialized as the atom `0`"
                        ),
                    },
                ));
            }
            components.push(Vec::new());
        } else {
            let additive = AdditiveDelay::from_timing_expr(expression.clone())
                .map_err(|error| symbolic_error(provenance.span().clone(), error))?;
            components.push(additive.terms().to_vec());
        }
    }
    Ok(Some(match components.as_slice() {
        [value] => PlacementDelay::One(value.clone()),
        [rise, fall] => PlacementDelay::Two {
            rise: rise.clone(),
            fall: fall.clone(),
        },
        [rise, fall, turn_off] => PlacementDelay::Three {
            rise: rise.clone(),
            fall: fall.clone(),
            turn_off: turn_off.clone(),
        },
        _ => unreachable!("DelayTuple has one, two, or three components"),
    }))
}

fn is_zero_timing_atom(expression: &TimingExpr) -> bool {
    expression.as_expr() == &Expr::Atom("0".to_string())
}

fn validate_analysis_inputs(
    graph: &TimingGraph,
    cut_graph: &CutTimingGraph,
    report: &TimingAnalysisReport,
) -> Result<(), DecompositionError> {
    let graph_nodes = graph.nodes().cloned().collect::<Vec<_>>();
    let consistent = graph_nodes == report.nodes()
        && graph.dependencies() == report.dependencies()
        && graph.constraints() == report.constraints()
        && cut_graph.dependencies() == report.cut_dependencies()
        && cut_graph.excluded_state_boundaries() == report.excluded_state_boundaries()
        && cut_graph.excluded_resolved_net_boundaries()
            == report.excluded_resolved_net_boundaries()
        && cut_graph.topological_order() == report.cut_topological_order();
    if consistent {
        return Ok(());
    }
    Err(analysis_error(
        graph,
        DecompositionErrorKind::InconsistentAnalysis {
            detail: "graph, cut graph, and report snapshots differ".to_string(),
        },
    ))
}

fn enumerate_functional_paths(
    graph: &TimingGraph,
    report: &TimingAnalysisReport,
) -> Result<Vec<FunctionalPath>, DecompositionError> {
    let mut adjacency = BTreeMap::<TimingNodeId, Vec<usize>>::new();
    for (order, dependency) in graph.dependencies().iter().enumerate() {
        adjacency
            .entry(dependency.source())
            .or_default()
            .push(order);
    }
    let public_splits = report
        .target_groups()
        .iter()
        .map(|group| {
            (
                graph
                    .signal_id(group.group().target())
                    .expect("an analyzed target is a graph signal"),
                group.public_output_split(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut paths = Vec::new();
    for constraint in graph.constraints() {
        let target = graph
            .signal_id(constraint.target())
            .expect("constraint target was validated by Milestone 15");
        for control in constraint.controls() {
            let source = graph
                .signal_id(control.source().signal())
                .expect("constraint control was validated by Milestone 15");
            if source == target {
                return Err(DecompositionError::new(
                    control.source().span().clone(),
                    DecompositionErrorKind::AmbiguousBoundaryTraversal {
                        constraint_id: constraint.id(),
                        control_id: control.id(),
                        dependency_order: usize::MAX,
                    },
                ));
            }
            let first_path = paths.len();
            let mut nodes = vec![source];
            let mut dependencies = Vec::new();
            let mut visited = BTreeSet::from([source]);
            let mut context = PathEnumeration {
                graph,
                adjacency: &adjacency,
                public_splits: &public_splits,
                constraint,
                control_id: control.id(),
                target,
                output: &mut paths,
            };
            enumerate_path_dfs(
                &mut context,
                source,
                &mut nodes,
                &mut dependencies,
                &mut visited,
                false,
            )?;
            if paths.len() == first_path {
                return Err(DecompositionError::new(
                    control.source().span().clone(),
                    DecompositionErrorKind::MissingFunctionalPath {
                        constraint_id: constraint.id(),
                        control_id: control.id(),
                    },
                ));
            }
        }
    }
    for (id, path) in paths.iter_mut().enumerate() {
        path.public.id = DecompositionPathId(id);
    }
    Ok(paths)
}

struct PathEnumeration<'a, 'b> {
    graph: &'a TimingGraph,
    adjacency: &'a BTreeMap<TimingNodeId, Vec<usize>>,
    public_splits: &'a BTreeMap<TimingNodeId, PublicOutputSplit>,
    constraint: &'a TimingConstraint,
    control_id: TimingControlId,
    target: TimingNodeId,
    output: &'b mut Vec<FunctionalPath>,
}

type CandidateCoverageIndex = Vec<Vec<Vec<Vec<usize>>>>;

fn enumerate_path_dfs(
    context: &mut PathEnumeration<'_, '_>,
    current: TimingNodeId,
    nodes: &mut Vec<TimingNodeId>,
    dependencies: &mut Vec<usize>,
    visited: &mut BTreeSet<TimingNodeId>,
    crossed_state_boundary: bool,
) -> Result<(), DecompositionError> {
    if current == context.target {
        let sites = path_sites(
            context.graph,
            context.public_splits,
            context.target,
            dependencies,
        );
        context.output.push(FunctionalPath {
            public: DecompositionPath {
                id: DecompositionPathId(usize::MAX),
                constraint_id: context.constraint.id(),
                control_id: context.control_id,
                nodes: nodes.clone(),
                dependency_orders: dependencies.clone(),
                sites: sites.iter().map(|site| site.site.clone()).collect(),
            },
            sites,
        });
        return Ok(());
    }

    for &dependency_order in context
        .adjacency
        .get(&current)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let dependency = &context.graph.dependencies()[dependency_order];
        let next = dependency.target();
        if visited.contains(&next) {
            // Every ordinary combinational cycle was rejected by M15. A
            // revisit after a typed boundary therefore belongs to a later
            // state/resolution epoch and is deliberately not admitted.
            continue;
        }
        let is_state_boundary = dependency.edge().kind() == DependencyKind::StateBoundary;
        if is_state_boundary && crossed_state_boundary {
            // A second modeled-state update is a later transition epoch.
            // Resolved-net boundaries remain separately typed and do not
            // consume this state-epoch allowance.
            continue;
        }
        dependencies.push(dependency_order);
        nodes.push(next);
        visited.insert(next);
        enumerate_path_dfs(
            context,
            next,
            nodes,
            dependencies,
            visited,
            crossed_state_boundary || is_state_boundary,
        )?;
        visited.remove(&next);
        nodes.pop();
        dependencies.pop();
    }
    Ok(())
}

fn path_sites(
    graph: &TimingGraph,
    public_splits: &BTreeMap<TimingNodeId, PublicOutputSplit>,
    target: TimingNodeId,
    dependency_orders: &[usize],
) -> Vec<PathSite> {
    let mut sites = Vec::new();
    for (path_edge_index, &dependency_order) in dependency_orders.iter().enumerate() {
        let dependency = &graph.dependencies()[dependency_order];
        if dependency.edge().kind() != DependencyKind::StateControl {
            sites.push(PathSite {
                site: PlacementSite::DependencyEdge {
                    dependency_order,
                    source: dependency.source(),
                    target: dependency.target(),
                },
                orientation_to_target: compose_suffix_orientation(
                    graph,
                    &dependency_orders[path_edge_index..],
                ),
            });
        }
        if let Some(node) = graph.node(dependency.target())
            && let TimingNodeKind::Assignment(assignment) = node.kind()
        {
            sites.push(PathSite {
                site: PlacementSite::ExistingAssignment {
                    node: node.id(),
                    assignment_order: assignment.assignment_order(),
                },
                orientation_to_target: compose_suffix_orientation(
                    graph,
                    &dependency_orders[path_edge_index + 1..],
                ),
            });
        }
    }
    if public_splits.get(&target) == Some(&PublicOutputSplit::Candidate) {
        sites.push(PathSite {
            site: PlacementSite::PublicOutputSplit { signal: target },
            orientation_to_target: PathOrientation::Positive,
        });
    }
    sites
}

fn compose_suffix_orientation(graph: &TimingGraph, dependency_orders: &[usize]) -> PathOrientation {
    dependency_orders
        .iter()
        .fold(PathOrientation::Positive, |orientation, order| {
            let edge = graph.dependencies()[*order].edge();
            let sense = if edge.kind() == DependencyKind::StateControl {
                TimingSense::PositiveUnate
            } else {
                edge.sense()
            };
            match (orientation, sense) {
                (PathOrientation::Ambiguous, _) => PathOrientation::Ambiguous,
                (_, TimingSense::NonUnate | TimingSense::Conditional) => PathOrientation::Ambiguous,
                (PathOrientation::Positive, TimingSense::NegativeUnate) => {
                    PathOrientation::Negative
                }
                (PathOrientation::Negative, TimingSense::NegativeUnate) => {
                    PathOrientation::Positive
                }
                (orientation, TimingSense::PositiveUnate | TimingSense::StateControl) => {
                    orientation
                }
            }
        })
}

fn build_candidates(
    graph: &TimingGraph,
    _report: &TimingAnalysisReport,
    paths: &[FunctionalPath],
) -> Result<(Vec<Candidate>, Vec<DecompositionError>), DecompositionError> {
    let mut physical_values = Vec::<(PlacementSite, PlacementDelay, usize)>::new();
    let mut blockers = Vec::new();

    for (anchor_path_index, path) in paths.iter().enumerate() {
        let constraint = constraint_for_path(graph, path)?;
        let range_sets = tuple_range_sets(constraint.additive_delay());
        for path_site in &path.sites {
            for ranges in &range_sets {
                let target_delay =
                    PlacementDelay::from_component_terms(constraint.additive_delay(), ranges)
                        .map_err(|error| symbolic_error(constraint.span().clone(), error))?;
                if target_delay.is_zero_contribution() {
                    continue;
                }
                let Some(local_delay) = target_delay.oriented(path_site.orientation_to_target)
                else {
                    push_unique_blocker(
                        &mut blockers,
                        control_error(
                            graph,
                            path,
                            DecompositionErrorKind::UnrepresentableSense {
                                constraint_id: path.public.constraint_id,
                                control_id: path.public.control_id,
                                path_id: path.public.id,
                                component: 0,
                                site: path_site.site.clone(),
                            },
                        ),
                    );
                    continue;
                };
                if !physical_values
                    .iter()
                    .any(|(site, delay, _)| site == &path_site.site && delay == &local_delay)
                {
                    physical_values.push((path_site.site.clone(), local_delay, anchor_path_index));
                }
            }
        }
    }

    let mut candidates = Vec::new();
    for (site, delay, anchor_path_index) in physical_values {
        let affected = paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| {
                path.sites
                    .iter()
                    .find(|path_site| path_site.site == site)
                    .map(|path_site| (index, path_site))
            })
            .collect::<Vec<_>>();
        let mut effect_options = Vec::with_capacity(affected.len());
        let mut compatible = true;
        for (path_index, path_site) in &affected {
            let path = &paths[*path_index];
            let constraint = constraint_for_path(graph, path)?;
            if delay.len() != constraint.additive_delay().len() {
                push_unique_blocker(
                    &mut blockers,
                    incompatible_site_error(graph, &paths[anchor_path_index], path, site.clone()),
                );
                compatible = false;
                break;
            }
            let Some(target_delay) = delay.oriented(path_site.orientation_to_target) else {
                push_unique_blocker(
                    &mut blockers,
                    control_error(
                        graph,
                        path,
                        DecompositionErrorKind::UnrepresentableSense {
                            constraint_id: path.public.constraint_id,
                            control_id: path.public.control_id,
                            path_id: path.public.id,
                            component: 0,
                            site: site.clone(),
                        },
                    ),
                );
                compatible = false;
                break;
            };
            let options = matching_tuple_contributions(constraint.additive_delay(), &target_delay)
                .map_err(|error| symbolic_error(constraint.span().clone(), error))?;
            if options.is_empty() {
                push_unique_blocker(
                    &mut blockers,
                    incompatible_site_error(graph, &paths[anchor_path_index], path, site.clone()),
                );
                compatible = false;
                break;
            }
            effect_options.push((*path_index, options));
        }
        if !compatible || effect_options.is_empty() {
            continue;
        }

        let variant_count = bounded_cartesian_product_count(
            effect_options
                .iter()
                .map(|(_, contributions)| contributions.len()),
            65_536,
        )
        .map_err(|count| {
            let path = &paths[effect_options[0].0];
            control_error(
                graph,
                path,
                DecompositionErrorKind::CandidateSpaceTooLarge {
                    constraint_id: path.public.constraint_id,
                    control_id: path.public.control_id,
                    count,
                },
            )
        })?;
        let mut variants = Vec::with_capacity(variant_count);
        expand_effect_variants(&effect_options, 0, &mut Vec::new(), &mut variants);
        assert_eq!(
            variants.len(),
            variant_count,
            "checked Cartesian candidate count must equal deterministic expansion"
        );
        let preference = candidate_preference(&site, &affected, paths);
        for effects in variants {
            let candidate = Candidate {
                site: site.clone(),
                delay: delay.clone(),
                effects,
                preference,
            };
            if !candidates.iter().any(|existing| existing == &candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by(compare_candidates);
    Ok((candidates, blockers))
}

fn tuple_range_sets(tuple: &AdditiveDelayTuple) -> Vec<Vec<TermRange>> {
    let component_options = tuple
        .components()
        .map(component_range_options)
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    expand_ranges(&component_options, 0, &mut Vec::new(), &mut output);
    output
}

fn component_range_options(component: &AdditiveDelay) -> Vec<TermRange> {
    let mut ranges = vec![TermRange::new(0, 0).expect("empty range is valid")];
    for start in 0..component.len() {
        for end in start + 1..=component.len() {
            ranges.push(TermRange::new(start, end).expect("ordered source range is valid"));
        }
    }
    ranges
}

fn expand_ranges(
    options: &[Vec<TermRange>],
    index: usize,
    current: &mut Vec<TermRange>,
    output: &mut Vec<Vec<TermRange>>,
) {
    if index == options.len() {
        output.push(current.clone());
        return;
    }
    for &range in &options[index] {
        current.push(range);
        expand_ranges(options, index + 1, current, output);
        current.pop();
    }
}

fn matching_tuple_contributions(
    source: &AdditiveDelayTuple,
    target_delay: &PlacementDelay,
) -> Result<Vec<AdditiveDelayTupleContribution>, TimingTermsError> {
    if source.len() != target_delay.len() {
        return Ok(Vec::new());
    }
    let mut component_options = Vec::with_capacity(source.len());
    for component in 0..source.len() {
        let source_component = source
            .component(component)
            .expect("component is bounded by source arity");
        let terms = target_delay
            .component(component)
            .expect("component is bounded by target arity");
        let ranges = if terms.is_empty() {
            vec![TermRange::new(0, 0)?]
        } else {
            source_component.matching_ranges(terms)?
        };
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        component_options.push(ranges);
    }
    let mut range_sets = Vec::new();
    expand_ranges(&component_options, 0, &mut Vec::new(), &mut range_sets);
    range_sets
        .iter()
        .map(|ranges| source.select_ranges(ranges))
        .collect()
}

fn expand_effect_variants(
    options: &[(usize, Vec<AdditiveDelayTupleContribution>)],
    index: usize,
    current: &mut Vec<CandidateEffect>,
    output: &mut Vec<Vec<CandidateEffect>>,
) {
    if index == options.len() {
        output.push(current.clone());
        return;
    }
    let (path_index, contributions) = &options[index];
    for contribution in contributions {
        current.push(CandidateEffect {
            path_index: *path_index,
            contribution: contribution.clone(),
        });
        expand_effect_variants(options, index + 1, current, output);
        current.pop();
    }
}

/// Returns the exact Cartesian-product size when it is within `limit`.
///
/// Counting always finishes before allocation. Arithmetic overflow is reported
/// deterministically as `usize::MAX`, which is necessarily over every
/// representable allocation limit used by the planner.
fn bounded_cartesian_product_count(
    counts: impl IntoIterator<Item = usize>,
    limit: usize,
) -> Result<usize, usize> {
    let mut count = 1_usize;
    for factor in counts {
        count = match count.checked_mul(factor) {
            Some(count) => count,
            None => return Err(usize::MAX),
        };
    }
    if count > limit { Err(count) } else { Ok(count) }
}

fn candidate_preference(
    site: &PlacementSite,
    affected: &[(usize, &PathSite)],
    paths: &[FunctionalPath],
) -> CandidatePreference {
    let controls = affected
        .iter()
        .map(|(path_index, _)| {
            (
                paths[*path_index].public.constraint_id,
                paths[*path_index].public.control_id,
            )
        })
        .collect::<BTreeSet<_>>();
    let shared = affected.len() > 1
        && controls.iter().all(|identity| {
            paths
                .iter()
                .filter(|path| (path.public.constraint_id, path.public.control_id) == *identity)
                .all(|path| path.sites.iter().any(|path_site| &path_site.site == site))
        });
    let target_distance = affected
        .iter()
        .map(|(path_index, _)| {
            let path = &paths[*path_index];
            path.sites
                .iter()
                .position(|path_site| &path_site.site == site)
                .map_or(usize::MAX, |position| path.sites.len() - position - 1)
        })
        .max()
        .unwrap_or(usize::MAX);
    let first_path = affected
        .iter()
        .map(|(path_index, _)| &paths[*path_index].public)
        .min_by_key(|path| {
            (
                path.constraint_id.ordinal(),
                path.control_id.ordinal(),
                path.id.ordinal(),
            )
        })
        .expect("a candidate affects at least one path");
    CandidatePreference {
        kind: match site {
            PlacementSite::ExistingAssignment { .. } => 0,
            PlacementSite::DependencyEdge { .. } | PlacementSite::PublicOutputSplit { .. } => 1,
        },
        non_shared: !shared,
        target_distance,
        constraint_order: first_path.constraint_id.ordinal(),
        control_order: first_path.control_id.ordinal(),
        path_order: first_path.id.ordinal(),
        site_order: placement_site_order(site),
    }
}

fn placement_site_order(site: &PlacementSite) -> usize {
    match site {
        PlacementSite::ExistingAssignment {
            assignment_order, ..
        } => assignment_order.saturating_mul(3).saturating_add(1),
        PlacementSite::DependencyEdge {
            dependency_order, ..
        } => dependency_order.saturating_mul(3),
        PlacementSite::PublicOutputSplit { signal } => {
            (usize::MAX / 2).saturating_add(signal.ordinal() as usize)
        }
    }
}

fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    left.preference
        .kind
        .cmp(&right.preference.kind)
        .then_with(|| left.preference.non_shared.cmp(&right.preference.non_shared))
        .then_with(|| right.effects.len().cmp(&left.effects.len()))
        .then_with(|| {
            left.preference
                .target_distance
                .cmp(&right.preference.target_distance)
        })
        .then_with(|| {
            (
                left.preference.constraint_order,
                left.preference.control_order,
                left.preference.path_order,
                left.preference.site_order,
            )
                .cmp(&(
                    right.preference.constraint_order,
                    right.preference.control_order,
                    right.preference.path_order,
                    right.preference.site_order,
                ))
        })
        .then_with(|| left.site.cmp(&right.site))
        .then_with(|| compare_placement_delay(&left.delay, &right.delay))
        .then_with(|| compare_effects(&left.effects, &right.effects))
}

fn compare_placement_delay(left: &PlacementDelay, right: &PlacementDelay) -> Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| {
            let left_terms = left.components().map(<[DelayTerm]>::len).sum::<usize>();
            let right_terms = right.components().map(<[DelayTerm]>::len).sum::<usize>();
            right_terms.cmp(&left_terms)
        })
        .then_with(|| {
            (0..left.len())
                .map(|component| {
                    compare_terms(
                        left.component(component)
                            .expect("component is bounded by tuple arity"),
                        right
                            .component(component)
                            .expect("equal arity follows the primary comparison"),
                    )
                })
                .find(|ordering| *ordering != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        })
}

fn compare_terms(left: &[DelayTerm], right: &[DelayTerm]) -> Ordering {
    right.len().cmp(&left.len()).then_with(|| {
        left.iter()
            .zip(right)
            .map(|(left, right)| {
                crate::serialize::render_timing_expr(left.as_timing_expr()).cmp(
                    &crate::serialize::render_timing_expr(right.as_timing_expr()),
                )
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    })
}

fn compare_effects(left: &[CandidateEffect], right: &[CandidateEffect]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| {
        left.iter()
            .zip(right)
            .map(|(left, right)| {
                left.path_index.cmp(&right.path_index).then_with(|| {
                    contribution_positions(&left.contribution)
                        .cmp(&contribution_positions(&right.contribution))
                })
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    })
}

fn contribution_positions(contribution: &AdditiveDelayTupleContribution) -> Vec<Vec<usize>> {
    contribution
        .components()
        .map(|component| component.positions().to_vec())
        .collect()
}

#[derive(Default, Debug)]
struct SearchStats {
    print: bool,
    calls: u64,
    backtracks: u64,
    max_depth: usize,

    candidate_checks: u64,
    rejected_selected_site: u64,
    rejected_not_fitting: u64,
    candidate_fits_calls: u64,

    most_shared_calls: u64,
    most_shared_time: Duration,

    start: Option<Instant>,
    last_report: Option<Instant>,
}

fn solve_exact_cover(
    graph: &TimingGraph,
    paths: &[FunctionalPath],
    candidates: &[Candidate],
) -> Result<Vec<usize>, DecompositionError> {
    let mut covered = paths
        .iter()
        .map(|path| {
            let constraint = &graph.constraints()[path.public.constraint_id.ordinal() as usize];
            constraint
                .additive_delay()
                .components()
                .map(|component| vec![false; component.len()])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let coverage_index = build_candidate_coverage_index(&covered, candidates);
    let mut selected = Vec::new();
    let mut selected_sites = BTreeSet::new();
    if search_exact_cover(
        paths,
        candidates,
        &mut covered,
        &mut selected,
        &mut selected_sites,
        &coverage_index,
        &mut SearchStats::default(),
    ) {
        selected.sort_by(|left, right| compare_candidates(&candidates[*left], &candidates[*right]));
        return Ok(selected);
    }

    let (path_index, component, term_position) = first_uncovered(&covered).unwrap_or((0, 0, 0));
    let path = &paths[path_index];
    Err(control_error(
        graph,
        path,
        DecompositionErrorKind::NoExactCover {
            constraint_id: path.public.constraint_id,
            control_id: path.public.control_id,
            component,
            term_position,
        },
    ))
}

fn search_exact_cover(
    paths: &[FunctionalPath],
    candidates: &[Candidate],
    covered: &mut [Vec<Vec<bool>>],
    selected: &mut Vec<usize>,
    selected_sites: &mut BTreeSet<PlacementSite>,
    coverage_index: &CandidateCoverageIndex,
    stats: &mut SearchStats,
) -> bool {
    stats.calls += 1;

    if selected.len() > stats.max_depth {
        stats.max_depth = selected.len();

        if stats.print {
            eprintln!(
                "NEW DEPTH: {} (calls={}, elapsed={:.1}s)",
                stats.max_depth,
                stats.calls,
                stats.start.unwrap().elapsed().as_secs_f64(),
            );
        }
    }

    if stats.start.is_none() {
        stats.start = Some(Instant::now());
        stats.last_report = stats.start;
    }

    if stats.print && stats.calls % 10_000 == 0 {
        let elapsed = stats.start.unwrap().elapsed();

        eprintln!(
            "search: elapsed={:5.1}s calls={:>7} most_shared: {:5.1}s depth={:>2} backtracks={:>7} cand_checks={:>10} rej_sel={:>10} rej_fit={:>8} cand_fits={:>8}",
            elapsed.as_secs_f64(),
            stats.calls,
            stats.most_shared_time.as_secs_f64(),
            selected.len(),
            stats.backtracks,
            stats.candidate_checks,
            stats.rejected_selected_site,
            stats.rejected_not_fitting,
            stats.candidate_fits_calls,
        );
    }

    let before = Instant::now();

    let mut fit_cache = vec![None; candidates.len()];
    let choice = most_shared_uncovered(
        paths,
        candidates,
        covered,
        selected,
        selected_sites,
        &coverage_index,
        &mut fit_cache,
        stats,
    );

    stats.most_shared_calls += 1;
    stats.most_shared_time += before.elapsed();

    let Some((path_index, component, term_position)) = choice else {
        return true;
    };

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        if selected_sites.contains(&candidate.site)
            || !candidate_covers(candidate, path_index, component, term_position)
            || !candidate_fits(candidate, paths, candidates, covered, selected)
        {
            continue;
        }
        let previous = covered.to_vec();
        apply_candidate(candidate, covered);
        selected.push(candidate_index);
        selected_sites.insert(candidate.site.clone());
        if search_exact_cover(
            paths,
            candidates,
            covered,
            selected,
            selected_sites,
            coverage_index,
            stats,
        ) {
            return true;
        }
        selected_sites.remove(&candidate.site);
        selected.pop();
        covered.clone_from_slice(&previous);

        stats.backtracks += 1;
    }
    false
}

fn build_candidate_coverage_index(
    covered: &[Vec<Vec<bool>>],
    candidates: &[Candidate],
) -> CandidateCoverageIndex {
    let mut index: CandidateCoverageIndex = covered
        .iter()
        .map(|components| {
            components
                .iter()
                .map(|terms| vec![Vec::<usize>::new(); terms.len()])
                .collect()
        })
        .collect::<Vec<_>>();

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        for effect in &candidate.effects {
            for (component, contribution) in effect.contribution.components().enumerate() {
                for &position in contribution.positions() {
                    index[effect.path_index][component][position].push(candidate_index);
                }
            }
        }
    }

    index
}

/// Chooses the next uncovered term by the greatest number of functional paths
/// one currently compatible candidate can cover. This lets an overlapping
/// suffix reserve its shared site before source-specific prefixes consume the
/// same terms. Equal breadth retains source/path/component order.
fn most_shared_uncovered(
    paths: &[FunctionalPath],
    candidates: &[Candidate],
    covered: &[Vec<Vec<bool>>],
    selected: &[usize],
    selected_sites: &BTreeSet<PlacementSite>,
    coverage_index: &CandidateCoverageIndex,
    fit_cache: &mut [Option<bool>],
    stats: &mut SearchStats,
) -> Option<(usize, usize, usize)> {
    let mut best = None;
    let mut best_breadth = 0;

    for (path_index, components) in covered.iter().enumerate() {
        for (component, terms) in components.iter().enumerate() {
            for (term_position, is_covered) in terms.iter().enumerate() {
                if *is_covered {
                    continue;
                }

                let breadth = coverage_index[path_index][component][term_position]
                    .iter()
                    .filter_map(|&candidate_index| {
                        let candidate = &candidates[candidate_index];

                        stats.candidate_checks += 1;
                        if selected_sites.contains(&candidate.site) {
                            stats.rejected_selected_site += 1;
                            return None;
                        }

                        let fits = match fit_cache[candidate_index] {
                            Some(value) => value,
                            None => {
                                stats.candidate_fits_calls += 1;
                                let value = candidate_fits(
                                    &candidates[candidate_index],
                                    paths,
                                    candidates,
                                    covered,
                                    selected,
                                );

                                fit_cache[candidate_index] = Some(value);
                                value
                            }
                        };
                        if !fits {
                            stats.rejected_not_fitting += 1;
                            return None;
                        }

                        Some(candidate.effects.len())
                    })
                    .max()
                    .unwrap_or(0);

                if best.is_none() || breadth > best_breadth {
                    best_breadth = breadth;
                    best = Some((path_index, component, term_position));
                }
            }
        }
    }

    best
}

fn first_uncovered(covered: &[Vec<Vec<bool>>]) -> Option<(usize, usize, usize)> {
    covered.iter().enumerate().find_map(|(path, components)| {
        components
            .iter()
            .enumerate()
            .find_map(|(component, terms)| {
                terms
                    .iter()
                    .position(|covered| !covered)
                    .map(|term| (path, component, term))
            })
    })
}

fn candidate_covers(
    candidate: &Candidate,
    path_index: usize,
    component: usize,
    term_position: usize,
) -> bool {
    candidate
        .effects
        .iter()
        .find(|effect| effect.path_index == path_index)
        .and_then(|effect| effect.contribution.component(component))
        .is_some_and(|contribution| contribution.positions().contains(&term_position))
}

fn candidate_fits(
    candidate: &Candidate,
    paths: &[FunctionalPath],
    candidates: &[Candidate],
    covered: &[Vec<Vec<bool>>],
    selected: &[usize],
) -> bool {
    // First check that this candidate doesn't overlap anything already covered.
    for effect in &candidate.effects {
        for (component, contribution) in effect.contribution.components().enumerate() {
            if contribution
                .positions()
                .iter()
                .any(|&position| covered[effect.path_index][component][position])
            {
                return false;
            }
        }
    }

    for effect in &candidate.effects {
        let path_index = effect.path_index;
        let path = &paths[path_index];
        let candidate_order = path
            .sites
            .iter()
            .position(|site| site.site == candidate.site)
            .expect("candidate effect implies that its site is on the path");

        for component in 0..effect.contribution.len() {
            let candidate_positions = effect
                .contribution
                .component(component)
                .expect("component is bounded by contribution arity")
                .positions();

            // Empty contributions don't participate in the ordering.
            if candidate_positions.is_empty() {
                continue;
            }

            // The candidate's own contribution must be strictly ordered.
            if candidate_positions
                .windows(2)
                .any(|window| window[0] >= window[1])
            {
                return false;
            }

            let mut previous: Option<&[usize]> = None;
            let mut next: Option<&[usize]> = None;

            let mut previous_order = 0;
            let mut next_order = usize::MAX;

            for &selected_index in selected {
                let selected_candidate = &candidates[selected_index];

                let Some(selected_effect) = selected_candidate
                    .effects
                    .iter()
                    .find(|effect| effect.path_index == path_index)
                else {
                    continue;
                };

                let Some(selected_contribution) = selected_effect.contribution.component(component)
                else {
                    continue;
                };

                let selected_positions = selected_contribution.positions();

                // Empty contributions don't affect the concatenated sequence.
                if selected_positions.is_empty() {
                    continue;
                }

                let selected_order = path
                    .sites
                    .iter()
                    .position(|site| site.site == selected_candidate.site)
                    .expect("selected candidate site must be on the path");

                if selected_order < candidate_order && selected_order > previous_order {
                    previous_order = selected_order;
                    previous = Some(selected_positions);
                }

                if selected_order > candidate_order && selected_order < next_order {
                    next_order = selected_order;
                    next = Some(selected_positions);
                }
            }

            if let Some(previous) = previous {
                if previous.last().unwrap() >= candidate_positions.first().unwrap() {
                    return false;
                }
            }

            if let Some(next) = next {
                if candidate_positions.last().unwrap() >= next.first().unwrap() {
                    return false;
                }
            }
        }
    }

    true
}

fn apply_candidate(candidate: &Candidate, covered: &mut [Vec<Vec<bool>>]) {
    for effect in &candidate.effects {
        for (component, contribution) in effect.contribution.components().enumerate() {
            for &position in contribution.positions() {
                covered[effect.path_index][component][position] = true;
            }
        }
    }
}

fn validate_placement_sites(
    graph: &TimingGraph,
    report: &TimingAnalysisReport,
    decomposition: &Decomposition,
) -> Result<(), DecompositionError> {
    let mut seen = BTreeSet::new();
    for placement in &decomposition.placements {
        if !seen.insert(placement.site.clone()) {
            return Err(site_error(
                graph,
                report,
                &placement.site,
                DecompositionErrorKind::DuplicatePlacementSite {
                    site: placement.site.clone(),
                },
            ));
        }
        let valid = match &placement.site {
            PlacementSite::ExistingAssignment {
                node,
                assignment_order,
            } => graph.node(*node).is_some_and(|node| {
                matches!(
                    node.kind(),
                    TimingNodeKind::Assignment(assignment)
                        if assignment.assignment_order() == *assignment_order
                )
            }),
            PlacementSite::DependencyEdge {
                dependency_order,
                source,
                target,
            } => graph
                .dependencies()
                .get(*dependency_order)
                .is_some_and(|dependency| {
                    dependency.source() == *source
                        && dependency.target() == *target
                        && dependency.edge().kind() != DependencyKind::StateControl
                }),
            PlacementSite::PublicOutputSplit { signal } => {
                let typed_output = graph.node(*signal).is_some_and(|node| match node.kind() {
                    TimingNodeKind::Signal(signal) => {
                        signal.has_role(TimingSignalRole::Output)
                            || signal.has_role(TimingSignalRole::Inout)
                    }
                    TimingNodeKind::Assignment(_) => false,
                });
                typed_output
                    && report.target_groups().iter().any(|group| {
                        graph.signal_id(group.group().target()) == Some(*signal)
                            && group.public_output_split() == PublicOutputSplit::Candidate
                    })
            }
        };
        if !valid {
            return Err(site_error(
                graph,
                report,
                &placement.site,
                DecompositionErrorKind::StalePlacementSite {
                    site: placement.site.clone(),
                },
            ));
        }
    }
    Ok(())
}

fn constraint_for_path<'a>(
    graph: &'a TimingGraph,
    path: &FunctionalPath,
) -> Result<&'a TimingConstraint, DecompositionError> {
    graph
        .constraints()
        .get(path.public.constraint_id.ordinal() as usize)
        .filter(|constraint| {
            constraint
                .controls()
                .iter()
                .any(|control| control.id() == path.public.control_id)
        })
        .ok_or_else(|| {
            analysis_error(
                graph,
                DecompositionErrorKind::InconsistentAnalysis {
                    detail: format!(
                        "path {} refers to missing constraint/control",
                        path.public.id
                    ),
                },
            )
        })
}

fn verified_components(tuple: &DelayTuple) -> Vec<VerifiedDelayComponent> {
    match tuple {
        DelayTuple::One(_) => vec![VerifiedDelayComponent::All],
        DelayTuple::Two { .. } => vec![VerifiedDelayComponent::Rise, VerifiedDelayComponent::Fall],
        DelayTuple::Three { .. } => vec![
            VerifiedDelayComponent::Rise,
            VerifiedDelayComponent::Fall,
            VerifiedDelayComponent::TurnOff,
        ],
    }
}

fn push_unique_blocker(blockers: &mut Vec<DecompositionError>, blocker: DecompositionError) {
    if !blockers.contains(&blocker) {
        blockers.push(blocker);
    }
}

fn earliest_blocker(mut blockers: Vec<DecompositionError>) -> DecompositionError {
    blockers.sort_by(|left, right| {
        compare_span_values(left.span(), right.span())
            .then_with(|| blocker_priority(left.kind()).cmp(&blocker_priority(right.kind())))
    });
    blockers
        .into_iter()
        .next()
        .expect("the caller checks that at least one blocker exists")
}

const fn blocker_priority(kind: &DecompositionErrorKind) -> u8 {
    match kind {
        DecompositionErrorKind::UnrepresentableSense { .. } => 0,
        DecompositionErrorKind::IncompatibleSiteValues { .. } => 1,
        _ => 2,
    }
}

fn incompatible_site_error(
    graph: &TimingGraph,
    first: &FunctionalPath,
    conflicting: &FunctionalPath,
    site: PlacementSite,
) -> DecompositionError {
    let first_span = control_span(graph, first);
    let conflicting_span = control_span(graph, conflicting);
    let span = if compare_span_values(&first_span, &conflicting_span) == Ordering::Greater {
        conflicting_span
    } else {
        first_span
    };
    DecompositionError::new(
        span,
        DecompositionErrorKind::IncompatibleSiteValues {
            site,
            first_constraint_id: first.public.constraint_id,
            first_control_id: first.public.control_id,
            conflicting_constraint_id: conflicting.public.constraint_id,
            conflicting_control_id: conflicting.public.control_id,
        },
    )
}

fn control_span(graph: &TimingGraph, path: &FunctionalPath) -> Span {
    let constraint = &graph.constraints()[path.public.constraint_id.ordinal() as usize];
    constraint
        .controls()
        .iter()
        .find(|control| control.id() == path.public.control_id)
        .map(|control| control.source().span().clone())
        .unwrap_or_else(|| constraint.span().clone())
}

fn analysis_error(graph: &TimingGraph, kind: DecompositionErrorKind) -> DecompositionError {
    DecompositionError::new(analysis_span(graph), kind)
}

fn analysis_span(graph: &TimingGraph) -> Span {
    graph
        .nodes()
        .map(|node| node.span())
        .min_by(compare_spans)
        .cloned()
        .unwrap_or_else(|| Span::new("<timing-decompose>", 1, 1))
}

fn control_id_error(
    graph: &TimingGraph,
    constraint: &TimingConstraint,
    control_id: TimingControlId,
    kind: DecompositionErrorKind,
) -> DecompositionError {
    let span = constraint
        .controls()
        .iter()
        .find(|control| control.id() == control_id)
        .map(|control| control.source().span().clone())
        .unwrap_or_else(|| constraint.span().clone());
    let _ = graph;
    DecompositionError::new(span, kind)
}

fn control_error(
    graph: &TimingGraph,
    path: &FunctionalPath,
    kind: DecompositionErrorKind,
) -> DecompositionError {
    let constraint = &graph.constraints()[path.public.constraint_id.ordinal() as usize];
    control_id_error(graph, constraint, path.public.control_id, kind)
}

fn site_error(
    graph: &TimingGraph,
    report: &TimingAnalysisReport,
    site: &PlacementSite,
    kind: DecompositionErrorKind,
) -> DecompositionError {
    let span = match site {
        PlacementSite::ExistingAssignment { node, .. } => {
            graph.node(*node).map(|node| node.span().clone())
        }
        PlacementSite::DependencyEdge {
            dependency_order, ..
        } => graph
            .dependencies()
            .get(*dependency_order)
            .map(|dependency| dependency.edge().span().clone()),
        PlacementSite::PublicOutputSplit { signal } => {
            graph.node(*signal).map(|node| node.span().clone())
        }
    }
    .or_else(|| report.nodes().first().map(|node| node.span().clone()))
    .unwrap_or_else(|| Span::new("<timing-decompose>", 1, 1));
    DecompositionError::new(span, kind)
}

fn symbolic_error(span: Span, error: TimingTermsError) -> DecompositionError {
    DecompositionError::new(
        span,
        DecompositionErrorKind::SymbolicTerms {
            detail: error.to_string(),
        },
    )
}

fn compare_spans(left: &&Span, right: &&Span) -> Ordering {
    compare_span_values(left, right)
}

fn compare_span_values(left: &Span, right: &Span) -> Ordering {
    left.path
        .to_string_lossy()
        .cmp(&right.path.to_string_lossy())
        .then_with(|| left.line.cmp(&right.line))
        .then_with(|| left.column.cmp(&right.column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ValueOperator;
    use crate::timing_graph::{
        AssignmentFunction, DependencyEdge, TimingConstraintSource, TimingControlSource,
        TimingSignalRole, analyze_timing_graph, cut_register_cycles,
    };

    fn span(line: usize) -> Span {
        Span::new("decompose.sv", line, 1)
    }

    fn roles(values: &[TimingSignalRole]) -> BTreeSet<TimingSignalRole> {
        values.iter().copied().collect()
    }

    fn atom(value: &str) -> TimingExpr {
        TimingExpr::atom(value).unwrap()
    }

    fn add_terms(values: &[&str]) -> TimingExpr {
        match values {
            [value] => atom(value),
            _ => TimingExpr::operation(
                TimingOperator::Add,
                values.iter().map(|value| atom(value)).collect(),
            )
            .unwrap(),
        }
    }

    fn one(value: &str) -> DelayTuple {
        DelayTuple::One(atom(value))
    }

    fn one_terms(terms: &[&str]) -> DelayTuple {
        DelayTuple::One(add_terms(terms))
    }

    fn two(rise: &[&str], fall: &[&str]) -> DelayTuple {
        DelayTuple::Two {
            rise: add_terms(rise),
            fall: add_terms(fall),
        }
    }

    fn three(rise: &str, fall: &str, turn_off: &str) -> DelayTuple {
        DelayTuple::Three {
            rise: atom(rise),
            fall: atom(fall),
            turn_off: atom(turn_off),
        }
    }

    fn add_signal(
        graph: &mut TimingGraph,
        name: &str,
        signal_roles: &[TimingSignalRole],
        line: usize,
    ) -> TimingNodeId {
        graph
            .add_signal(name, roles(signal_roles), span(line))
            .unwrap()
    }

    fn add_assignment(
        graph: &mut TimingGraph,
        order: usize,
        target: &str,
        function: AssignmentFunction,
        operands: &[(TimingNodeId, usize, TimingSense)],
        boundary: Option<DependencyKind>,
        line: usize,
    ) -> TimingNodeId {
        let assignment = graph
            .add_assignment(order, target, function, span(line))
            .unwrap();
        for (source, operand, sense) in operands {
            graph
                .add_dependency(
                    *source,
                    assignment,
                    DependencyEdge::operand(*operand, *sense, span(line)).unwrap(),
                )
                .unwrap();
        }
        let target_node = graph.signal_id(target).unwrap();
        let edge = match boundary {
            None | Some(DependencyKind::Drive) => DependencyEdge::drive(span(line)),
            Some(DependencyKind::StateBoundary) => DependencyEdge::state_boundary(span(line)),
            Some(DependencyKind::ResolvedNetBoundary) => {
                DependencyEdge::resolved_net_boundary(span(line))
            }
            Some(DependencyKind::Operand | DependencyKind::StateControl) => unreachable!(),
        };
        graph.add_dependency(assignment, target_node, edge).unwrap();
        assignment
    }

    fn add_constraint(
        graph: &mut TimingGraph,
        control: &str,
        target: &str,
        delay: DelayTuple,
        line: usize,
    ) {
        let order = graph.constraints().len();
        graph
            .add_constraint(
                TimingConstraintSource::new(
                    order,
                    vec![TimingControlSource::new(control, None, span(line)).unwrap()],
                    target,
                    delay,
                    span(line),
                )
                .unwrap(),
            )
            .unwrap();
    }

    fn analyzed(graph: &TimingGraph) -> (CutTimingGraph, TimingAnalysisReport) {
        let cut = cut_register_cycles(graph).unwrap();
        let report = analyze_timing_graph(graph, &cut).unwrap();
        (cut, report)
    }

    fn decompose(graph: &TimingGraph) -> Decomposition {
        let (cut, report) = analyzed(graph);
        decompose_timing(graph, &cut, &report).unwrap()
    }

    fn assignment_site_target(graph: &TimingGraph, site: &PlacementSite) -> Option<String> {
        let PlacementSite::ExistingAssignment { node, .. } = site else {
            return None;
        };
        match graph.node(*node).unwrap().kind() {
            TimingNodeKind::Assignment(assignment) => Some(assignment.target().to_string()),
            TimingNodeKind::Signal(_) => None,
        }
    }

    fn term_atom(term: &DelayTerm) -> &str {
        match term.as_timing_expr().as_expr() {
            crate::ir::Expr::Atom(atom) => atom,
            crate::ir::Expr::List(_) => panic!("expected atom term"),
        }
    }

    #[test]
    fn single_path_prefers_the_existing_assignment_and_verifies() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            3,
        );
        add_constraint(&mut graph, "a", "y", one("T"), 4);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 1);
        assert_eq!(decomposition.placements().len(), 1);
        assert_eq!(
            assignment_site_target(&graph, decomposition.placements()[0].site()).as_deref(),
            Some("y")
        );
        assert_eq!(decomposition.verification().paths().len(), 1);
    }

    #[test]
    fn distinct_controls_share_the_nearest_common_suffix_assignment() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "n", &[TimingSignalRole::Internal], 3);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 4);
        let n_assignment = add_assignment(
            &mut graph,
            0,
            "n",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (a, 0, TimingSense::PositiveUnate),
                (b, 1, TimingSense::PositiveUnate),
            ],
            None,
            5,
        );
        let n = graph.signal_id("n").unwrap();
        add_assignment(
            &mut graph,
            1,
            "y",
            AssignmentFunction::DirectAtom,
            &[(n, 0, TimingSense::PositiveUnate)],
            None,
            6,
        );
        add_constraint(&mut graph, "a", "y", one("T"), 7);
        add_constraint(&mut graph, "b", "y", one("T"), 8);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 1);
        assert_eq!(
            assignment_site_target(&graph, decomposition.placements()[0].site()).as_deref(),
            Some("y")
        );
        assert_ne!(
            decomposition.placements()[0].site(),
            &PlacementSite::ExistingAssignment {
                node: n_assignment,
                assignment_order: 0,
            }
        );
    }

    #[test]
    fn overlapping_branch_prefixes_leave_one_shared_suffix_on_the_output() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 3);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (a, 0, TimingSense::PositiveUnate),
                (b, 1, TimingSense::PositiveUnate),
            ],
            None,
            4,
        );
        add_constraint(
            &mut graph,
            "a",
            "y",
            two(&["A_rise", "Y_rise"], &["A_fall", "Y_fall"]),
            5,
        );
        add_constraint(
            &mut graph,
            "b",
            "y",
            two(&["B_rise", "Y_rise"], &["B_fall", "Y_fall"]),
            6,
        );

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 3);

        let branch_terms = |source| {
            decomposition
                .placements()
                .iter()
                .find(|placement| {
                    matches!(
                        placement.site(),
                        PlacementSite::DependencyEdge {
                            source: candidate_source,
                            ..
                        } if *candidate_source == source
                    )
                })
                .map(|placement| {
                    placement
                        .delay()
                        .components()
                        .map(|terms| terms.iter().map(term_atom).collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        assert_eq!(branch_terms(a), vec![vec!["A_rise"], vec!["A_fall"]]);
        assert_eq!(branch_terms(b), vec![vec!["B_rise"], vec!["B_fall"]]);

        let shared = decomposition
            .placements()
            .iter()
            .find(|placement| {
                assignment_site_target(&graph, placement.site()).as_deref() == Some("y")
            })
            .unwrap();
        assert_eq!(shared.coverage().len(), 2);
        assert_eq!(
            shared
                .delay()
                .components()
                .map(|terms| terms.iter().map(term_atom).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec!["Y_rise"], vec!["Y_fall"]]
        );
    }

    #[test]
    fn one_control_to_multiple_targets_keeps_the_common_prefix_shared() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "n", &[TimingSignalRole::Internal], 2);
        add_signal(&mut graph, "q", &[TimingSignalRole::Output], 3);
        add_signal(&mut graph, "r", &[TimingSignalRole::Output], 4);
        add_assignment(
            &mut graph,
            0,
            "n",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            5,
        );
        let n = graph.signal_id("n").unwrap();
        add_assignment(
            &mut graph,
            1,
            "q",
            AssignmentFunction::DirectAtom,
            &[(n, 0, TimingSense::PositiveUnate)],
            None,
            6,
        );
        add_assignment(
            &mut graph,
            2,
            "r",
            AssignmentFunction::DirectAtom,
            &[(n, 0, TimingSense::PositiveUnate)],
            None,
            7,
        );
        add_constraint(&mut graph, "a", "q", one_terms(&["A", "Q"]), 8);
        add_constraint(&mut graph, "a", "r", one_terms(&["A", "R"]), 9);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 3);
        let placement_terms = |target| {
            decomposition
                .placements()
                .iter()
                .find(|placement| {
                    assignment_site_target(&graph, placement.site()).as_deref() == Some(target)
                })
                .map(|placement| {
                    placement
                        .delay()
                        .components()
                        .next()
                        .unwrap()
                        .iter()
                        .map(term_atom)
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        assert_eq!(placement_terms("n"), vec!["A"]);
        assert_eq!(placement_terms("q"), vec!["Q"]);
        assert_eq!(placement_terms("r"), vec!["R"]);
    }

    #[test]
    fn one_path_keeps_a_full_multi_term_tuple_on_one_existing_assignment() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            3,
        );
        add_constraint(&mut graph, "a", "y", one_terms(&["A", "B"]), 4);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 1);
        assert_eq!(
            assignment_site_target(&graph, decomposition.placements()[0].site()).as_deref(),
            Some("y")
        );
        assert_eq!(
            decomposition.placements()[0]
                .delay()
                .components()
                .next()
                .unwrap()
                .iter()
                .map(term_atom)
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    #[test]
    fn uncovered_term_without_a_candidate_remains_a_search_failure() {
        let mut covered = vec![vec![vec![false]]];
        let mut selected = Vec::new();
        let mut selected_sites = BTreeSet::new();
        assert_eq!(
            most_shared_uncovered(
                &[],
                &[],
                &covered,
                &selected,
                &selected_sites,
                &build_candidate_coverage_index(&covered, &[]),
                &mut vec![],
                &mut SearchStats::default()
            ),
            Some((0, 0, 0))
        );
        let coverage_index = build_candidate_coverage_index(&covered, &[]);
        assert!(!search_exact_cover(
            &[],
            &[],
            &mut covered,
            &mut selected,
            &mut selected_sites,
            &coverage_index,
            &mut SearchStats::default(),
        ));
    }

    #[test]
    fn one_control_to_multiple_targets_prefers_a_shared_prefix_assignment() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "n", &[TimingSignalRole::Internal], 2);
        add_signal(&mut graph, "q", &[TimingSignalRole::Output], 3);
        add_signal(&mut graph, "r", &[TimingSignalRole::Output], 4);
        let prefix = add_assignment(
            &mut graph,
            0,
            "n",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            5,
        );
        let n = graph.signal_id("n").unwrap();
        for (order, target) in [(1, "q"), (2, "r")] {
            add_assignment(
                &mut graph,
                order,
                target,
                AssignmentFunction::DirectAtom,
                &[(n, 0, TimingSense::PositiveUnate)],
                None,
                6 + order,
            );
            add_constraint(&mut graph, "a", target, one("T"), 10 + order);
        }

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 1);
        assert_eq!(
            decomposition.placements()[0].site(),
            &PlacementSite::ExistingAssignment {
                node: prefix,
                assignment_order: 0,
            }
        );
        assert_eq!(decomposition.placements()[0].coverage().len(), 2);
    }

    #[test]
    fn differing_control_delays_use_source_specific_dependency_edges() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 3);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (a, 0, TimingSense::PositiveUnate),
                (b, 1, TimingSense::PositiveUnate),
            ],
            None,
            4,
        );
        add_constraint(&mut graph, "a", "y", one("Ta"), 5);
        add_constraint(&mut graph, "b", "y", one("Tb"), 6);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 2);
        assert!(
            decomposition
                .placements()
                .iter()
                .all(|placement| matches!(placement.site(), PlacementSite::DependencyEdge { .. }))
        );
        let sources = decomposition
            .placements()
            .iter()
            .map(|placement| match placement.site() {
                PlacementSite::DependencyEdge { source, .. } => *source,
                _ => unreachable!(),
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(sources, BTreeSet::from([a, b]));
    }

    #[test]
    fn negative_suffix_swaps_rise_and_fall_but_retains_turn_off() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "n", &[TimingSignalRole::Internal], 3);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 4);
        add_assignment(
            &mut graph,
            0,
            "n",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (a, 0, TimingSense::PositiveUnate),
                (b, 1, TimingSense::PositiveUnate),
            ],
            None,
            5,
        );
        let n = graph.signal_id("n").unwrap();
        add_assignment(
            &mut graph,
            1,
            "y",
            AssignmentFunction::Operator(ValueOperator::Not),
            &[(n, 0, TimingSense::NegativeUnate)],
            None,
            6,
        );
        add_constraint(&mut graph, "a", "y", three("Ra", "Fa", "Za"), 7);
        add_constraint(&mut graph, "b", "y", three("Rb", "Fb", "Zb"), 8);

        let decomposition = decompose(&graph);
        let a_placement = decomposition
            .placements()
            .iter()
            .find(|placement| {
                matches!(
                    placement.site(),
                    PlacementSite::DependencyEdge { source, .. } if *source == a
                )
            })
            .unwrap();
        let PlacementDelay::Three {
            rise,
            fall,
            turn_off,
        } = a_placement.delay()
        else {
            panic!("expected three-entry placement")
        };
        assert_eq!(term_atom(&rise[0]), "Fa");
        assert_eq!(term_atom(&fall[0]), "Ra");
        assert_eq!(term_atom(&turn_off[0]), "Za");
    }

    #[test]
    fn state_control_path_crosses_one_boundary_and_uses_state_assignment() {
        let mut graph = TimingGraph::new();
        let clk = add_signal(&mut graph, "clk", &[TimingSignalRole::Input], 1);
        add_signal(
            &mut graph,
            "q",
            &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
            2,
        );
        let assignment = graph
            .add_assignment(0, "q", AssignmentFunction::DirectAtom, span(3))
            .unwrap();
        graph
            .add_dependency(
                clk,
                assignment,
                DependencyEdge::state_control(Some(crate::timing_graph::Transition::Rise), span(3)),
            )
            .unwrap();
        graph
            .add_dependency(
                assignment,
                graph.signal_id("q").unwrap(),
                DependencyEdge::state_boundary(span(3)),
            )
            .unwrap();
        add_constraint(&mut graph, "clk", "q", one("Tclk"), 4);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 1);
        assert_eq!(
            decomposition.placements()[0].site(),
            &PlacementSite::ExistingAssignment {
                node: assignment,
                assignment_order: 0,
            }
        );
    }

    #[test]
    fn state_feedback_is_epoch_bounded_without_losing_the_derived_output_path() {
        let mut graph = TimingGraph::new();
        let clk = add_signal(&mut graph, "clk", &[TimingSignalRole::Input], 1);
        let q = add_signal(&mut graph, "q", &[TimingSignalRole::ModeledRegister], 2);
        add_signal(&mut graph, "q_n", &[TimingSignalRole::Output], 3);
        let state_assignment = graph
            .add_assignment(0, "q", AssignmentFunction::DirectAtom, span(4))
            .unwrap();
        graph
            .add_dependency(
                q,
                state_assignment,
                DependencyEdge::operand(0, TimingSense::PositiveUnate, span(4)).unwrap(),
            )
            .unwrap();
        graph
            .add_dependency(
                clk,
                state_assignment,
                DependencyEdge::state_control(Some(crate::timing_graph::Transition::Rise), span(4)),
            )
            .unwrap();
        graph
            .add_dependency(state_assignment, q, DependencyEdge::state_boundary(span(4)))
            .unwrap();
        add_assignment(
            &mut graph,
            1,
            "q_n",
            AssignmentFunction::Operator(ValueOperator::Not),
            &[(q, 0, TimingSense::NegativeUnate)],
            None,
            5,
        );
        add_constraint(&mut graph, "clk", "q_n", one("T"), 6);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 1);
        assert_eq!(
            assignment_site_target(&graph, decomposition.placements()[0].site()).as_deref(),
            Some("q_n")
        );
    }

    #[test]
    fn one_resolved_net_boundary_is_retained_on_a_valid_path() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        let r = add_signal(
            &mut graph,
            "r",
            &[TimingSignalRole::Internal, TimingSignalRole::ResolvedNet],
            3,
        );
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 4);
        for (order, source) in [(0, a), (1, b)] {
            add_assignment(
                &mut graph,
                order,
                "r",
                AssignmentFunction::DirectAtom,
                &[(source, 0, TimingSense::PositiveUnate)],
                Some(DependencyKind::ResolvedNetBoundary),
                5 + order,
            );
        }
        add_assignment(
            &mut graph,
            2,
            "y",
            AssignmentFunction::DirectAtom,
            &[(r, 0, TimingSense::PositiveUnate)],
            None,
            8,
        );
        add_constraint(&mut graph, "a", "y", one("T"), 9);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 1);
        assert!(
            decomposition.paths()[0]
                .dependency_orders()
                .iter()
                .any(|order| graph.dependencies()[*order].edge().kind()
                    == DependencyKind::ResolvedNetBoundary)
        );
    }

    #[test]
    fn a_second_state_boundary_is_cut_while_a_direct_same_epoch_route_remains() {
        let mut graph = TimingGraph::new();
        let clk = add_signal(&mut graph, "clk", &[TimingSignalRole::Input], 1);
        let q0 = add_signal(&mut graph, "q0", &[TimingSignalRole::ModeledRegister], 2);
        let q1 = add_signal(
            &mut graph,
            "q1",
            &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
            3,
        );
        let first = graph
            .add_assignment(0, "q0", AssignmentFunction::DirectAtom, span(4))
            .unwrap();
        graph
            .add_dependency(
                clk,
                first,
                DependencyEdge::state_control(Some(crate::timing_graph::Transition::Rise), span(4)),
            )
            .unwrap();
        graph
            .add_dependency(first, q0, DependencyEdge::state_boundary(span(4)))
            .unwrap();
        let second = graph
            .add_assignment(
                1,
                "q1",
                AssignmentFunction::Operator(ValueOperator::Or),
                span(5),
            )
            .unwrap();
        graph
            .add_dependency(
                q0,
                second,
                DependencyEdge::operand(0, TimingSense::PositiveUnate, span(5)).unwrap(),
            )
            .unwrap();
        graph
            .add_dependency(
                clk,
                second,
                DependencyEdge::operand(1, TimingSense::PositiveUnate, span(5)).unwrap(),
            )
            .unwrap();
        graph
            .add_dependency(second, q1, DependencyEdge::state_boundary(span(5)))
            .unwrap();
        add_constraint(&mut graph, "clk", "q1", one("T"), 6);
        let (cut, report) = analyzed(&graph);

        let decomposition = decompose_timing(&graph, &cut, &report).unwrap();
        assert_eq!(decomposition.paths().len(), 1);
        assert_eq!(
            decomposition.paths()[0]
                .dependency_orders()
                .iter()
                .filter(|order| graph.dependencies()[**order].edge().is_state_boundary())
                .count(),
            1
        );
        verify_decomposition(&graph, &cut, &report, &decomposition).unwrap();
    }

    #[test]
    fn reconvergent_control_is_checked_on_every_parallel_functional_path() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "n0", &[TimingSignalRole::Internal], 2);
        add_signal(&mut graph, "n1", &[TimingSignalRole::Internal], 3);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 4);
        for (order, target) in [(0, "n0"), (1, "n1")] {
            add_assignment(
                &mut graph,
                order,
                target,
                AssignmentFunction::DirectAtom,
                &[(a, 0, TimingSense::PositiveUnate)],
                None,
                5 + order,
            );
        }
        let n0 = graph.signal_id("n0").unwrap();
        let n1 = graph.signal_id("n1").unwrap();
        add_assignment(
            &mut graph,
            2,
            "y",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (n0, 0, TimingSense::PositiveUnate),
                (n1, 1, TimingSense::PositiveUnate),
            ],
            None,
            8,
        );
        add_constraint(&mut graph, "a", "y", one("T"), 9);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 2);
        assert_eq!(decomposition.placements().len(), 1);
        assert_eq!(decomposition.placements()[0].coverage().len(), 2);
        assert_eq!(
            assignment_site_target(&graph, decomposition.placements()[0].site()).as_deref(),
            Some("y")
        );
    }

    #[test]
    fn parallel_dependency_occurrences_produce_distinct_path_identities() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::Operator(ValueOperator::Or),
            &[
                (a, 0, TimingSense::PositiveUnate),
                (a, 1, TimingSense::PositiveUnate),
            ],
            None,
            3,
        );
        add_constraint(&mut graph, "a", "y", one("T"), 4);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.paths().len(), 2);
        assert_ne!(
            decomposition.paths()[0].dependency_orders()[0],
            decomposition.paths()[1].dependency_orders()[0]
        );
        assert_eq!(decomposition.placements()[0].coverage().len(), 2);
    }

    #[test]
    fn incompatible_values_on_every_shared_site_have_no_exact_cover() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            3,
        );
        add_constraint(&mut graph, "a", "y", one("Ta"), 4);
        add_constraint(&mut graph, "a", "y", one("Tb"), 5);
        let (cut, report) = analyzed(&graph);

        let error = decompose_timing(&graph, &cut, &report).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::IncompatibleSiteValues { .. }
        ));
        assert_eq!(error.span(), &span(4));
    }

    #[test]
    fn non_unate_source_specific_placement_rejects_distinct_transition_values() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 3);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::Operator(ValueOperator::Xor),
            &[(a, 0, TimingSense::NonUnate), (b, 1, TimingSense::NonUnate)],
            None,
            4,
        );
        add_constraint(&mut graph, "a", "y", three("Ra", "Fa", "Za"), 5);
        add_constraint(&mut graph, "b", "y", three("Rb", "Fb", "Zb"), 6);
        let (cut, report) = analyzed(&graph);

        let error = decompose_timing(&graph, &cut, &report).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::UnrepresentableSense { component: 0, .. }
        ));
        assert_eq!(error.span(), &span(5));
    }

    #[test]
    fn non_unate_source_specific_placement_is_exact_when_transition_values_match() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        let b = add_signal(&mut graph, "b", &[TimingSignalRole::Input], 2);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 3);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::Operator(ValueOperator::Xor),
            &[(a, 0, TimingSense::NonUnate), (b, 1, TimingSense::NonUnate)],
            None,
            4,
        );
        add_constraint(&mut graph, "a", "y", three("Ta", "Ta", "Za"), 5);
        add_constraint(&mut graph, "b", "y", three("Tb", "Tb", "Zb"), 6);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 2);
        assert!(
            decomposition
                .placements()
                .iter()
                .all(|placement| matches!(placement.site(), PlacementSite::DependencyEdge { .. }))
        );
        assert_eq!(decomposition.verification().paths().len(), 2);
    }

    #[test]
    fn public_output_split_is_the_only_isolated_site_for_an_internally_read_output() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "q", &[TimingSignalRole::Output], 2);
        add_signal(&mut graph, "q_n", &[TimingSignalRole::Output], 3);
        add_assignment(
            &mut graph,
            0,
            "q",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            4,
        );
        let q = graph.signal_id("q").unwrap();
        add_assignment(
            &mut graph,
            1,
            "q_n",
            AssignmentFunction::Operator(ValueOperator::Not),
            &[(q, 0, TimingSense::NegativeUnate)],
            None,
            5,
        );
        add_constraint(&mut graph, "a", "q", one("Tq"), 6);
        add_constraint(&mut graph, "a", "q_n", one("Tqn"), 7);

        let decomposition = decompose(&graph);
        assert_eq!(decomposition.placements().len(), 2);
        let q_split = decomposition
            .placements()
            .iter()
            .find(|placement| {
                matches!(
                    placement.site(),
                    PlacementSite::PublicOutputSplit { signal } if *signal == q
                )
            })
            .expect("q-only timing requires the typed public split");
        let PlacementDelay::One(q_terms) = q_split.delay() else {
            panic!("q constraint has one timing component")
        };
        assert_eq!(q_terms.len(), 1);
        assert_eq!(term_atom(&q_terms[0]), "Tq");
        assert_eq!(q_split.coverage().len(), 1);
        assert_eq!(q_split.coverage()[0].constraint_id().ordinal(), 0);

        let q_n_local = decomposition
            .placements()
            .iter()
            .find(|placement| {
                assignment_site_target(&graph, placement.site()).as_deref() == Some("q_n")
            })
            .expect("derived output timing remains local to q_n");
        let PlacementDelay::One(q_n_terms) = q_n_local.delay() else {
            panic!("q_n constraint has one timing component")
        };
        assert_eq!(q_n_terms.len(), 1);
        assert_eq!(term_atom(&q_n_terms[0]), "Tqn");
        assert_eq!(q_n_local.coverage().len(), 1);
        assert_eq!(q_n_local.coverage()[0].constraint_id().ordinal(), 1);

        assert!(
            decomposition
                .placements()
                .iter()
                .all(
                    |placement| assignment_site_target(&graph, placement.site()).as_deref()
                        != Some("q")
                ),
            "q's original assignment must not carry q-only public timing"
        );
    }

    #[test]
    fn independent_verifier_reports_reorder_gap_duplicate_and_stale_sites() {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            3,
        );
        add_constraint(
            &mut graph,
            "a",
            "y",
            DelayTuple::One(add_terms(&["A", "B"])),
            4,
        );
        let (cut, report) = analyzed(&graph);
        let accepted = decompose_timing(&graph, &cut, &report).unwrap();

        let mut reordered = accepted.clone();
        let PlacementDelay::One(terms) = &mut reordered.placements[0].delay else {
            unreachable!()
        };
        terms.reverse();
        let error = verify_decomposition(&graph, &cut, &report, &reordered).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::ReconstructionMismatch {
                component: 0,
                term_position: 0,
                ..
            }
        ));

        let mut gap = accepted.clone();
        let PlacementDelay::One(terms) = &mut gap.placements[0].delay else {
            unreachable!()
        };
        terms.pop();
        let error = verify_decomposition(&graph, &cut, &report, &gap).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::UncoveredTerm {
                component: 0,
                term_position: 1,
                ..
            }
        ));

        let mut duplicate = accepted.clone();
        duplicate.placements.push(duplicate.placements[0].clone());
        let error = verify_decomposition(&graph, &cut, &report, &duplicate).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::DuplicatePlacementSite { .. }
        ));

        let mut coverage = accepted.clone();
        coverage.placements[0].coverage.clear();
        let error = verify_decomposition(&graph, &cut, &report, &coverage).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::CoverageMismatch { .. }
        ));

        let mut wrong_arity = accepted.clone();
        wrong_arity.placements[0].delay = PlacementDelay::Two {
            rise: Vec::new(),
            fall: Vec::new(),
        };
        let error = verify_decomposition(&graph, &cut, &report, &wrong_arity).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::PlacementTupleArity {
                expected: 1,
                actual: 2,
                ..
            }
        ));

        let mut stale = accepted;
        stale.placements[0].site = PlacementSite::DependencyEdge {
            dependency_order: usize::MAX,
            source: a,
            target: a,
        };
        let error = verify_decomposition(&graph, &cut, &report, &stale).unwrap_err();
        assert!(matches!(
            error.kind(),
            DecompositionErrorKind::StalePlacementSite { .. }
        ));
    }

    fn deterministic_graph(unused_order: [&str; 2]) -> TimingGraph {
        let mut graph = TimingGraph::new();
        let a = add_signal(&mut graph, "a", &[TimingSignalRole::Input], 1);
        add_signal(&mut graph, "y", &[TimingSignalRole::Output], 2);
        add_assignment(
            &mut graph,
            0,
            "y",
            AssignmentFunction::DirectAtom,
            &[(a, 0, TimingSense::PositiveUnate)],
            None,
            3,
        );
        for (offset, name) in unused_order.into_iter().enumerate() {
            add_signal(&mut graph, name, &[TimingSignalRole::Internal], 20 + offset);
        }
        add_constraint(&mut graph, "a", "y", one("T"), 4);
        graph
    }

    #[test]
    fn candidate_product_is_rejected_before_expansion_at_a_deterministic_exact_count() {
        assert_eq!(
            bounded_cartesian_product_count([256, 256], 65_536),
            Ok(65_536)
        );
        let first = bounded_cartesian_product_count([256, 257], 65_536);
        let repeated = bounded_cartesian_product_count([256, 257], 65_536);
        assert_eq!(first, Err(65_792));
        assert_eq!(first, repeated);
        assert_eq!(
            bounded_cartesian_product_count([usize::MAX, 2], 65_536),
            Err(usize::MAX)
        );
    }

    #[test]
    fn dependency_source_order_is_the_final_candidate_tie_breaker() {
        let mut graph = TimingGraph::new();
        let first = add_signal(&mut graph, "first", &[TimingSignalRole::Input], 1);
        let second = add_signal(&mut graph, "second", &[TimingSignalRole::Input], 2);
        let third = add_signal(&mut graph, "third", &[TimingSignalRole::Internal], 3);
        let fourth = add_signal(&mut graph, "fourth", &[TimingSignalRole::Internal], 4);
        let earlier_site = PlacementSite::DependencyEdge {
            dependency_order: 3,
            source: first,
            target: third,
        };
        let later_site = PlacementSite::DependencyEdge {
            dependency_order: 4,
            source: second,
            target: fourth,
        };
        let tied = CandidatePreference {
            kind: 1,
            non_shared: true,
            target_distance: 2,
            constraint_order: 5,
            control_order: 6,
            path_order: 7,
            site_order: placement_site_order(&earlier_site),
        };
        let earlier = Candidate {
            site: earlier_site.clone(),
            delay: PlacementDelay::One(vec![
                DelayTerm::from_timing_expr(atom("z_source_first")).unwrap(),
            ]),
            effects: Vec::new(),
            preference: tied,
        };
        let later = Candidate {
            site: later_site.clone(),
            delay: PlacementDelay::One(vec![
                DelayTerm::from_timing_expr(atom("a_lexically_first")).unwrap(),
            ]),
            effects: Vec::new(),
            preference: CandidatePreference {
                site_order: placement_site_order(&later_site),
                ..tied
            },
        };
        let mut candidates = [later, earlier];
        candidates.sort_by(compare_candidates);

        assert_eq!(candidates[0].site, earlier_site);
        assert_eq!(
            candidates[0].preference.site_order,
            3 * 3,
            "dependency order wins before structural delay text"
        );
    }

    #[test]
    fn planning_and_verification_are_repeatable_and_ignore_unrelated_insertion_order() {
        let first_graph = deterministic_graph(["unused_a", "unused_b"]);
        let second_graph = deterministic_graph(["unused_b", "unused_a"]);
        let first = decompose(&first_graph);
        let repeated = decompose(&first_graph);
        let second = decompose(&second_graph);

        assert_eq!(first, repeated);
        assert_eq!(first, second);
    }
}
