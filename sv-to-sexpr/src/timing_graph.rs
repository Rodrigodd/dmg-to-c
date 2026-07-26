//! Deterministic data model for functional timing analysis.
//!
//! The graph intentionally separates signal nodes from emitted assignment
//! nodes. Durable IDs are allocated in source/build order and hide petgraph's
//! internal indices, so diagnostics and reports never depend on allocator or
//! traversal details. The full graph retains modeled-state and resolved-net
//! boundaries for path analysis; the deterministic cut graph excludes those
//! two typed boundary classes separately for SCC/topological analysis.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use petgraph::algo::{dominators::simple_fast, has_path_connecting, kosaraju_scc, toposort};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::Reversed;

use crate::diagnostic::{Diagnostic, Span};
use crate::ir::{Cell, CellItem, DelayTuple, Expr, ValueOperator};
use crate::timing_terms::AdditiveDelayTuple;

macro_rules! ordered_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            pub const fn ordinal(self) -> u32 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}{}", $prefix, self.0)
            }
        }
    };
}

ordered_id!(TimingNodeId, "n");
ordered_id!(TimingConstraintId, "p");
ordered_id!(TimingControlId, "c");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingNode {
    id: TimingNodeId,
    kind: TimingNodeKind,
    span: Span,
}

impl TimingNode {
    pub const fn id(&self) -> TimingNodeId {
        self.id
    }

    pub fn kind(&self) -> &TimingNodeKind {
        &self.kind
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimingNodeKind {
    Signal(SignalNode),
    Assignment(AssignmentNode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalNode {
    name: String,
    roles: BTreeSet<TimingSignalRole>,
}

impl SignalNode {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn roles(&self) -> &BTreeSet<TimingSignalRole> {
        &self.roles
    }

    pub fn has_role(&self, role: TimingSignalRole) -> bool {
        self.roles.contains(&role)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimingSignalRole {
    Input,
    Output,
    Inout,
    /// A source-declared resolved net (`inout`, `wire`, or `tri`).
    ///
    /// This is source type information, not an inference from driver count.
    ResolvedNet,
    ModeledRegister,
    Internal,
    Temporary,
    /// A deterministic `dN` signal introduced only to carry an exact timing
    /// placement or a raw/public split.
    TimingTemporary,
    /// A signal introduced by a resolved physical topology overlay.
    ///
    /// This is neither a logical temporary nor a timing identity signal.
    TopologyTemporary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentNode {
    assignment_order: usize,
    target: String,
    function: AssignmentFunction,
}

impl AssignmentNode {
    pub const fn assignment_order(&self) -> usize {
        self.assignment_order
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub const fn function(&self) -> AssignmentFunction {
        self.function
    }
}

/// The flat value shape represented by an assignment/operation node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentFunction {
    DirectAtom,
    Operator(ValueOperator),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// A signal used at a particular operand position of an assignment value.
    Operand,
    /// An ordinary assignment result driving its target signal.
    Drive,
    /// An assignment result updating modeled state. This is cut independently
    /// from any resolved-net boundary when SCCs/topological order are computed.
    StateBoundary,
    /// An assignment result entering a source-declared resolved net with more
    /// than one emitted driver. Resolution iteration is a graph boundary, not
    /// modeled state; the full graph retains this edge for path analysis.
    ResolvedNetBoundary,
    /// A sensitivity/event control affecting a modeled-state assignment.
    StateControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimingSense {
    PositiveUnate,
    NegativeUnate,
    NonUnate,
    Conditional,
    StateControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transition {
    Rise,
    Fall,
    TurnOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransitionEffect {
    Exact(Transition),
    Indeterminate,
}

/// Propagates a named transition through one functional dependency.
///
/// `TurnOff` is intentionally conservative: the converter does not invent a
/// simulator policy for how high-impedance transitions propagate through a
/// Boolean operation.
pub const fn propagate_transition(sense: TimingSense, transition: Transition) -> TransitionEffect {
    if matches!(transition, Transition::TurnOff) {
        return TransitionEffect::Indeterminate;
    }
    match sense {
        TimingSense::PositiveUnate | TimingSense::StateControl => {
            TransitionEffect::Exact(transition)
        }
        TimingSense::NegativeUnate => TransitionEffect::Exact(match transition {
            Transition::Rise => Transition::Fall,
            Transition::Fall => Transition::Rise,
            Transition::TurnOff => unreachable!(),
        }),
        TimingSense::NonUnate | TimingSense::Conditional => TransitionEffect::Indeterminate,
    }
}

/// Classifies the timing sense of a contracted value-operator operand.
///
/// `None` means the position is metadata rather than a signal dependency. The
/// caller must validate operator arity before using this function.
pub const fn classify_timing_sense(
    operator: ValueOperator,
    operand_index: usize,
) -> Option<TimingSense> {
    match operator {
        ValueOperator::Not => Some(TimingSense::NegativeUnate),
        ValueOperator::And | ValueOperator::Or => Some(TimingSense::PositiveUnate),
        ValueOperator::Nand | ValueOperator::Nor => Some(TimingSense::NegativeUnate),
        ValueOperator::Xor
        | ValueOperator::Xnor
        | ValueOperator::Eq
        | ValueOperator::CaseEq
        | ValueOperator::Neq
        | ValueOperator::CaseNeq => Some(TimingSense::NonUnate),
        ValueOperator::Mux => Some(if operand_index == 0 {
            TimingSense::NonUnate
        } else {
            TimingSense::Conditional
        }),
        ValueOperator::BufIf0 | ValueOperator::BufIf1 => Some(if operand_index == 0 {
            TimingSense::PositiveUnate
        } else {
            TimingSense::Conditional
        }),
        ValueOperator::DriveStrength => {
            if operand_index == 0 {
                Some(TimingSense::PositiveUnate)
            } else {
                None
            }
        }
        ValueOperator::BufIf0Strength | ValueOperator::BufIf1Strength => match operand_index {
            0 => Some(TimingSense::PositiveUnate),
            1 => Some(TimingSense::Conditional),
            _ => None,
        },
        ValueOperator::Keeper => None,
        ValueOperator::Nmos | ValueOperator::Pmos | ValueOperator::Rnmos => {
            Some(if operand_index == 0 {
                TimingSense::PositiveUnate
            } else {
                TimingSense::Conditional
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAssignmentOrigin {
    Continuous,
    Primitive,
    Keeper,
    ProceduralCombinational,
    ProceduralStateful,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOrigin {
    Source(SourceAssignmentOrigin),
    GeneratedTemporary { parent: SourceAssignmentOrigin },
    GeneratedTimingIdentity { parent: SourceAssignmentOrigin },
    GeneratedTopology { parent: SourceAssignmentOrigin },
}

impl AssignmentOrigin {
    pub const fn source(self) -> SourceAssignmentOrigin {
        match self {
            Self::Source(source)
            | Self::GeneratedTemporary { parent: source }
            | Self::GeneratedTimingIdentity { parent: source }
            | Self::GeneratedTopology { parent: source } => source,
        }
    }

    pub const fn is_temporary(self) -> bool {
        matches!(self, Self::GeneratedTemporary { .. })
    }

    pub const fn is_timing_identity(self) -> bool {
        matches!(self, Self::GeneratedTimingIdentity { .. })
    }

    pub const fn is_topology_generated(self) -> bool {
        matches!(self, Self::GeneratedTopology { .. })
    }

    pub const fn is_stateful_source(self) -> bool {
        matches!(
            self,
            Self::Source(SourceAssignmentOrigin::ProceduralStateful)
        )
    }
}

/// Typed origin of the delay tuple aligned with one emitted assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentDelayOrigin {
    ImplicitZero,
    GeneratedLogicalTemporaryZero,
    KeeperZero,
    ExplicitSourceDelay,
    PrimitiveSourceDelay,
    LegacySelectedSpecifyFallback,
    DecompositionPlacement,
    TopologyPlacement,
}

impl AssignmentDelayOrigin {
    pub const fn is_intrinsic_source_delay(self) -> bool {
        matches!(
            self,
            Self::ExplicitSourceDelay | Self::PrimitiveSourceDelay | Self::KeeperZero
        )
    }
}

/// Exact sensitivity/event provenance for a stateful source assignment.
///
/// A missing signal records an unrepresentable source event expression. The
/// compatibility lowerer may still return its Milestone 14 cell; functional
/// timing-graph construction rejects it at this exact span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateControlProvenance {
    signal: Option<String>,
    transition: Option<Transition>,
    span: Span,
}

impl StateControlProvenance {
    pub fn new(signal: String, transition: Option<Transition>, span: Span) -> Self {
        Self {
            signal: Some(signal),
            transition,
            span,
        }
    }

    pub fn unrepresentable(transition: Option<Transition>, span: Span) -> Self {
        Self {
            signal: None,
            transition,
            span,
        }
    }

    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }

    pub const fn transition(&self) -> Option<Transition> {
        self.transition
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

/// Provenance aligned one-for-one with emitted cell assignments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentProvenance {
    assignment_order: usize,
    source_assignment_order: usize,
    span: Span,
    origin: AssignmentOrigin,
    delay_origin: AssignmentDelayOrigin,
    state_controls: Vec<StateControlProvenance>,
}

impl AssignmentProvenance {
    pub fn new(
        assignment_order: usize,
        source_assignment_order: usize,
        span: Span,
        origin: AssignmentOrigin,
        state_controls: Vec<StateControlProvenance>,
    ) -> Result<Self, Diagnostic> {
        let delay_origin = match origin {
            AssignmentOrigin::GeneratedTemporary { .. } => {
                AssignmentDelayOrigin::GeneratedLogicalTemporaryZero
            }
            AssignmentOrigin::GeneratedTimingIdentity { .. } => {
                AssignmentDelayOrigin::DecompositionPlacement
            }
            AssignmentOrigin::GeneratedTopology { .. } => AssignmentDelayOrigin::TopologyPlacement,
            AssignmentOrigin::Source(SourceAssignmentOrigin::Keeper) => {
                AssignmentDelayOrigin::KeeperZero
            }
            AssignmentOrigin::Source(_) => AssignmentDelayOrigin::ImplicitZero,
        };
        Self::new_with_delay_origin(
            assignment_order,
            source_assignment_order,
            span,
            origin,
            delay_origin,
            state_controls,
        )
    }

    pub fn new_with_delay_origin(
        assignment_order: usize,
        source_assignment_order: usize,
        span: Span,
        origin: AssignmentOrigin,
        delay_origin: AssignmentDelayOrigin,
        state_controls: Vec<StateControlProvenance>,
    ) -> Result<Self, Diagnostic> {
        if !origin.is_stateful_source() && !state_controls.is_empty() {
            return Err(Diagnostic::error(
                span,
                "only a final stateful source assignment may retain event controls",
            ));
        }
        Ok(Self {
            assignment_order,
            source_assignment_order,
            span,
            origin,
            delay_origin,
            state_controls,
        })
    }

    pub const fn assignment_order(&self) -> usize {
        self.assignment_order
    }

    /// Stable source-driver identity shared by its generated temporaries and
    /// final emitted assignment.
    pub const fn source_assignment_order(&self) -> usize {
        self.source_assignment_order
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub const fn origin(&self) -> AssignmentOrigin {
        self.origin
    }

    pub const fn delay_origin(&self) -> AssignmentDelayOrigin {
        self.delay_origin
    }

    pub fn state_controls(&self) -> &[StateControlProvenance] {
        &self.state_controls
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingSignalMetadata {
    name: String,
    roles: BTreeSet<TimingSignalRole>,
    span: Span,
}

impl TimingSignalMetadata {
    pub fn new(
        name: String,
        roles: BTreeSet<TimingSignalRole>,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        if name.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing signal metadata name must be a non-empty atom",
            ));
        }
        if roles.is_empty() {
            return Err(Diagnostic::error(
                span,
                format!("timing signal metadata `{name}` must have at least one role"),
            ));
        }
        Ok(Self { name, roles, span })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn roles(&self) -> &BTreeSet<TimingSignalRole> {
        &self.roles
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    kind: DependencyKind,
    operand_index: Option<usize>,
    sense: TimingSense,
    event_transition: Option<Transition>,
    span: Span,
}

impl DependencyEdge {
    pub fn try_new(
        kind: DependencyKind,
        operand_index: Option<usize>,
        sense: TimingSense,
        event_transition: Option<Transition>,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        let valid = match kind {
            DependencyKind::Operand => {
                operand_index.is_some()
                    && sense != TimingSense::StateControl
                    && event_transition.is_none()
            }
            DependencyKind::Drive
            | DependencyKind::StateBoundary
            | DependencyKind::ResolvedNetBoundary => {
                operand_index.is_none()
                    && sense == TimingSense::PositiveUnate
                    && event_transition.is_none()
            }
            DependencyKind::StateControl => {
                operand_index.is_none() && sense == TimingSense::StateControl
            }
        };
        if !valid {
            return Err(Diagnostic::error(
                span,
                format!(
                    "invalid {kind:?} timing dependency: operand_index={operand_index:?}, sense={sense:?}, event_transition={event_transition:?}"
                ),
            ));
        }
        Ok(Self {
            kind,
            operand_index,
            sense,
            event_transition,
            span,
        })
    }

    pub fn operand(
        operand_index: usize,
        sense: TimingSense,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        Self::try_new(
            DependencyKind::Operand,
            Some(operand_index),
            sense,
            None,
            span,
        )
    }

    pub fn drive(span: Span) -> Self {
        Self {
            kind: DependencyKind::Drive,
            operand_index: None,
            sense: TimingSense::PositiveUnate,
            event_transition: None,
            span,
        }
    }

    pub fn state_boundary(span: Span) -> Self {
        Self {
            kind: DependencyKind::StateBoundary,
            operand_index: None,
            sense: TimingSense::PositiveUnate,
            event_transition: None,
            span,
        }
    }

    pub fn resolved_net_boundary(span: Span) -> Self {
        Self {
            kind: DependencyKind::ResolvedNetBoundary,
            operand_index: None,
            sense: TimingSense::PositiveUnate,
            event_transition: None,
            span,
        }
    }

    pub fn state_control(event_transition: Option<Transition>, span: Span) -> Self {
        Self {
            kind: DependencyKind::StateControl,
            operand_index: None,
            sense: TimingSense::StateControl,
            event_transition,
            span,
        }
    }

    pub const fn kind(&self) -> DependencyKind {
        self.kind
    }

    pub const fn operand_index(&self) -> Option<usize> {
        self.operand_index
    }

    pub const fn sense(&self) -> TimingSense {
        self.sense
    }

    pub const fn event_transition(&self) -> Option<Transition> {
        self.event_transition
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub const fn is_state_boundary(&self) -> bool {
        matches!(self.kind, DependencyKind::StateBoundary)
    }
}

/// A dependency record in deterministic insertion/source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyRecord {
    source: TimingNodeId,
    target: TimingNodeId,
    edge: DependencyEdge,
}

impl DependencyRecord {
    pub const fn source(&self) -> TimingNodeId {
        self.source
    }

    pub const fn target(&self) -> TimingNodeId {
        self.target
    }

    pub fn edge(&self) -> &DependencyEdge {
        &self.edge
    }
}

/// Source data for one scalar control before its durable ID is allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingControlSource {
    signal: String,
    transition: Option<Transition>,
    span: Span,
}

impl TimingControlSource {
    pub fn new(
        signal: impl Into<String>,
        transition: Option<Transition>,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        let signal = signal.into();
        if signal.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing control signal must be a non-empty atom",
            ));
        }
        Ok(Self {
            signal,
            transition,
            span,
        })
    }

    pub fn signal(&self) -> &str {
        &self.signal
    }

    pub const fn transition(&self) -> Option<Transition> {
        self.transition
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingControl {
    id: TimingControlId,
    order_in_path: usize,
    source: TimingControlSource,
}

impl TimingControl {
    pub const fn id(&self) -> TimingControlId {
        self.id
    }

    pub const fn order_in_path(&self) -> usize {
        self.order_in_path
    }

    pub fn source(&self) -> &TimingControlSource {
        &self.source
    }
}

/// One exact structural specify path and all of its scalar controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingConstraint {
    id: TimingConstraintId,
    path_order: usize,
    controls: Vec<TimingControl>,
    target: String,
    target_span: Span,
    delay: DelayTuple,
    additive_delay: AdditiveDelayTuple,
    span: Span,
}

impl TimingConstraint {
    pub const fn id(&self) -> TimingConstraintId {
        self.id
    }

    pub const fn path_order(&self) -> usize {
        self.path_order
    }

    pub fn controls(&self) -> &[TimingControl] {
        &self.controls
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn target_span(&self) -> &Span {
        &self.target_span
    }

    pub fn delay(&self) -> &DelayTuple {
        &self.delay
    }

    pub fn additive_delay(&self) -> &AdditiveDelayTuple {
        &self.additive_delay
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

/// Input to [`TimingGraph::add_constraint`], before path/control IDs exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingConstraintSource {
    path_order: usize,
    controls: Vec<TimingControlSource>,
    target: String,
    target_span: Span,
    delay: DelayTuple,
    additive_delay: AdditiveDelayTuple,
    span: Span,
}

impl TimingConstraintSource {
    pub fn from_constraint(constraint: &TimingConstraint) -> Self {
        Self {
            path_order: constraint.path_order,
            controls: constraint
                .controls
                .iter()
                .map(|control| control.source.clone())
                .collect(),
            target: constraint.target.clone(),
            target_span: constraint.target_span.clone(),
            delay: constraint.delay.clone(),
            additive_delay: constraint.additive_delay.clone(),
            span: constraint.span.clone(),
        }
    }

    pub fn new(
        path_order: usize,
        controls: Vec<TimingControlSource>,
        target: impl Into<String>,
        delay: DelayTuple,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_target_span(path_order, controls, target, delay, span.clone(), span)
    }

    pub fn new_with_target_span(
        path_order: usize,
        controls: Vec<TimingControlSource>,
        target: impl Into<String>,
        delay: DelayTuple,
        target_span: Span,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        let target = target.into();
        if controls.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing constraint must contain at least one scalar control",
            ));
        }
        if target.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing constraint target must be a non-empty atom",
            ));
        }
        delay.validate("timing constraint delay").map_err(|error| {
            Diagnostic::error(span.clone(), format!("invalid timing constraint: {error}"))
        })?;
        let additive_delay = AdditiveDelayTuple::from_delay_tuple(&delay).map_err(|error| {
            Diagnostic::error(
                span.clone(),
                format!("invalid additive timing constraint delay: {error}"),
            )
        })?;
        let rebuilt_delay = additive_delay.to_delay_tuple().map_err(|error| {
            Diagnostic::error(
                span.clone(),
                format!("cannot rebuild exact additive timing constraint delay: {error}"),
            )
        })?;
        if rebuilt_delay != delay {
            return Err(Diagnostic::error(
                span,
                "additive timing terms did not exactly rebuild their source delay tuple",
            ));
        }
        Ok(Self {
            path_order,
            controls,
            target,
            target_span,
            delay,
            additive_delay,
            span,
        })
    }

    pub const fn path_order(&self) -> usize {
        self.path_order
    }

    pub fn controls(&self) -> &[TimingControlSource] {
        &self.controls
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn target_span(&self) -> &Span {
        &self.target_span
    }

    pub fn delay(&self) -> &DelayTuple {
        &self.delay
    }

    pub fn additive_delay(&self) -> &AdditiveDelayTuple {
        &self.additive_delay
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetGroupKind {
    SinglePath,
    MultiplePaths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlGroupKind {
    SingleTarget,
    MultipleTargets,
}

/// Deterministic factual grouping used as the input to structural
/// reachability/dominance classification in a later phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingTargetGroup {
    target: String,
    constraint_ids: Vec<TimingConstraintId>,
    control_ids: Vec<TimingControlId>,
    kind: TargetGroupKind,
}

impl TimingTargetGroup {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn constraint_ids(&self) -> &[TimingConstraintId] {
        &self.constraint_ids
    }

    pub fn control_ids(&self) -> &[TimingControlId] {
        &self.control_ids
    }

    pub const fn kind(&self) -> TargetGroupKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicOutputSplit {
    NotPublic,
    NotRequired,
    Candidate,
}

/// A complete composed functional sense observed on at least one path from a
/// scalar specify control to its target.
///
/// State-control paths retain both their source event and the transition
/// effect after every subsequent dependency. An absent source event represents
/// a level-sensitive state control and therefore has no named transition
/// effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimingPathSense {
    PositiveUnate,
    NegativeUnate,
    NonUnate,
    Conditional,
    StateControl {
        event_transition: Option<Transition>,
        target_effect: Option<TransitionEffect>,
    },
}

/// Per-scalar-control facts normalized to durable node IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingControlReport {
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    source_node: TimingNodeId,
    target_node: TimingNodeId,
    reachable_nodes: Vec<TimingNodeId>,
    target_dominators: Vec<TimingNodeId>,
    target_post_dominators: Vec<TimingNodeId>,
    path_senses: Vec<TimingPathSense>,
}

impl TimingControlReport {
    pub const fn constraint_id(&self) -> TimingConstraintId {
        self.constraint_id
    }

    pub const fn control_id(&self) -> TimingControlId {
        self.control_id
    }

    pub const fn source_node(&self) -> TimingNodeId {
        self.source_node
    }

    pub const fn target_node(&self) -> TimingNodeId {
        self.target_node
    }

    /// Nodes on at least one source-to-target path, in durable graph order.
    pub fn reachable_nodes(&self) -> &[TimingNodeId] {
        &self.reachable_nodes
    }

    /// Nodes which dominate the target with this scalar control as root.
    ///
    /// This is the complete per-control chain, including the source and target
    /// endpoints. It is a factual slice for this one control/path record, not
    /// a cross-constraint "prefix" classification.
    pub fn target_dominators(&self) -> &[TimingNodeId] {
        &self.target_dominators
    }

    /// Nodes which post-dominate this scalar control with the target as the
    /// root of the reversed full graph.
    ///
    /// This is the complete per-control chain, including the source and target
    /// endpoints. It is a factual slice for this one control/path record, not
    /// a second name for a shared region.
    pub fn target_post_dominators(&self) -> &[TimingNodeId] {
        &self.target_post_dominators
    }

    pub fn path_senses(&self) -> &[TimingPathSense] {
        &self.path_senses
    }
}

/// Cross-constraint report for every specify control record with one scalar
/// source signal.
///
/// Constraint and control IDs preserve structural source order. Target nodes
/// are distinct endpoints in durable graph order. `common_prefix` contains
/// only nodes which dominate every distinct target from the common source;
/// the source and every terminal target are excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingControlGroupReport {
    control_signal: String,
    source_node: TimingNodeId,
    constraint_ids: Vec<TimingConstraintId>,
    control_ids: Vec<TimingControlId>,
    target_nodes: Vec<TimingNodeId>,
    kind: ControlGroupKind,
    common_prefix: Vec<TimingNodeId>,
}

impl TimingControlGroupReport {
    pub fn control_signal(&self) -> &str {
        &self.control_signal
    }

    pub const fn source_node(&self) -> TimingNodeId {
        self.source_node
    }

    pub fn constraint_ids(&self) -> &[TimingConstraintId] {
        &self.constraint_ids
    }

    pub fn control_ids(&self) -> &[TimingControlId] {
        &self.control_ids
    }

    pub fn target_nodes(&self) -> &[TimingNodeId] {
        &self.target_nodes
    }

    pub const fn kind(&self) -> ControlGroupKind {
        self.kind
    }

    pub fn common_prefix(&self) -> &[TimingNodeId] {
        &self.common_prefix
    }
}

/// Cross-constraint report for every specify path targeting one scalar signal.
///
/// Every vector uses durable IDs in deterministic graph/source order. The
/// `common_suffix` is computed across distinct scalar sources in the reversed
/// graph rooted at the common target. Source endpoints and the target endpoint
/// are excluded. This type contains no raw petgraph index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingTargetGroupReport {
    group: TimingTargetGroup,
    reachable_controls: Vec<TimingControlId>,
    control_reports: Vec<TimingControlReport>,
    common_suffix: Vec<TimingNodeId>,
    reconvergent_nodes: Vec<TimingNodeId>,
    public_output_split: PublicOutputSplit,
}

impl TimingTargetGroupReport {
    pub fn group(&self) -> &TimingTargetGroup {
        &self.group
    }

    pub fn reachable_controls(&self) -> &[TimingControlId] {
        &self.reachable_controls
    }

    pub fn control_reports(&self) -> &[TimingControlReport] {
        &self.control_reports
    }

    pub fn common_suffix(&self) -> &[TimingNodeId] {
        &self.common_suffix
    }

    pub fn reconvergent_nodes(&self) -> &[TimingNodeId] {
        &self.reconvergent_nodes
    }

    pub const fn public_output_split(&self) -> PublicOutputSplit {
        self.public_output_split
    }
}

/// Deterministic library-facing snapshot of the complete Milestone 15 timing
/// analysis. It intentionally contains no raw petgraph indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingAnalysisReport {
    nodes: Vec<TimingNode>,
    dependencies: Vec<DependencyRecord>,
    cut_dependencies: Vec<DependencyRecord>,
    excluded_state_boundaries: Vec<DependencyRecord>,
    excluded_resolved_net_boundaries: Vec<DependencyRecord>,
    cut_topological_order: Vec<TimingNodeId>,
    constraints: Vec<TimingConstraint>,
    control_groups: Vec<TimingControlGroupReport>,
    target_groups: Vec<TimingTargetGroupReport>,
}

impl TimingAnalysisReport {
    pub fn nodes(&self) -> &[TimingNode] {
        &self.nodes
    }

    pub fn dependencies(&self) -> &[DependencyRecord] {
        &self.dependencies
    }

    pub fn cut_dependencies(&self) -> &[DependencyRecord] {
        &self.cut_dependencies
    }

    pub fn excluded_state_boundaries(&self) -> &[DependencyRecord] {
        &self.excluded_state_boundaries
    }

    pub fn excluded_resolved_net_boundaries(&self) -> &[DependencyRecord] {
        &self.excluded_resolved_net_boundaries
    }

    pub fn cut_topological_order(&self) -> &[TimingNodeId] {
        &self.cut_topological_order
    }

    pub fn constraints(&self) -> &[TimingConstraint] {
        &self.constraints
    }

    pub fn control_groups(&self) -> &[TimingControlGroupReport] {
        &self.control_groups
    }

    pub fn target_groups(&self) -> &[TimingTargetGroupReport] {
        &self.target_groups
    }

    /// Renders a compact stable inspection form. Relative source paths remain
    /// normalized repository-relative provenance; absolute paths are reduced
    /// to their file name so a checkout location cannot leak into a report.
    pub fn render(&self) -> String {
        render_timing_analysis_report(self)
    }
}

/// Stable directed graph plus source-ordered records and exact constraints.
#[derive(Debug, Clone)]
pub struct TimingGraph {
    graph: StableDiGraph<TimingNode, DependencyEdge>,
    node_indices: BTreeMap<TimingNodeId, NodeIndex>,
    node_order: Vec<TimingNodeId>,
    signal_ids: BTreeMap<String, TimingNodeId>,
    assignment_ids: BTreeMap<usize, TimingNodeId>,
    dependencies: Vec<DependencyRecord>,
    constraints: Vec<TimingConstraint>,
    constraint_ids_by_path_order: BTreeMap<usize, TimingConstraintId>,
    constraint_ids_by_target: BTreeMap<String, Vec<TimingConstraintId>>,
    next_node_id: u32,
    next_constraint_id: u32,
    next_control_id: u32,
}

impl Default for TimingGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingGraph {
    pub fn new() -> Self {
        Self {
            graph: StableDiGraph::new(),
            node_indices: BTreeMap::new(),
            node_order: Vec::new(),
            signal_ids: BTreeMap::new(),
            assignment_ids: BTreeMap::new(),
            dependencies: Vec::new(),
            constraints: Vec::new(),
            constraint_ids_by_path_order: BTreeMap::new(),
            constraint_ids_by_target: BTreeMap::new(),
            next_node_id: 0,
            next_constraint_id: 0,
            next_control_id: 0,
        }
    }

    pub fn add_signal(
        &mut self,
        name: impl Into<String>,
        roles: BTreeSet<TimingSignalRole>,
        span: Span,
    ) -> Result<TimingNodeId, Diagnostic> {
        let name = name.into();
        if name.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing signal name must be a non-empty atom",
            ));
        }
        if roles.is_empty() {
            return Err(Diagnostic::error(
                span,
                format!("timing signal `{name}` must have at least one role"),
            ));
        }
        if self.signal_ids.contains_key(&name) {
            return Err(Diagnostic::error(
                span,
                format!("duplicate timing signal `{name}`"),
            ));
        }
        let kind = TimingNodeKind::Signal(SignalNode {
            name: name.clone(),
            roles,
        });
        let id = self.add_node(kind, span)?;
        self.signal_ids.insert(name, id);
        Ok(id)
    }

    pub fn add_assignment(
        &mut self,
        assignment_order: usize,
        target: impl Into<String>,
        function: AssignmentFunction,
        span: Span,
    ) -> Result<TimingNodeId, Diagnostic> {
        let target = target.into();
        if target.is_empty() {
            return Err(Diagnostic::error(
                span,
                "timing assignment target must be a non-empty atom",
            ));
        }
        if !self.signal_ids.contains_key(&target) {
            return Err(Diagnostic::error(
                span,
                format!("timing assignment target `{target}` is not a known signal"),
            ));
        }
        if self.assignment_ids.contains_key(&assignment_order) {
            return Err(Diagnostic::error(
                span,
                format!("duplicate timing assignment order {assignment_order}"),
            ));
        }
        let kind = TimingNodeKind::Assignment(AssignmentNode {
            assignment_order,
            target,
            function,
        });
        let id = self.add_node(kind, span)?;
        self.assignment_ids.insert(assignment_order, id);
        Ok(id)
    }

    fn add_node(&mut self, kind: TimingNodeKind, span: Span) -> Result<TimingNodeId, Diagnostic> {
        let id = TimingNodeId(self.next_node_id);
        self.next_node_id = self.next_node_id.checked_add(1).ok_or_else(|| {
            Diagnostic::error(span.clone(), "timing graph contains too many nodes")
        })?;
        let node = TimingNode { id, kind, span };
        let index = self.graph.add_node(node);
        self.node_indices.insert(id, index);
        self.node_order.push(id);
        Ok(id)
    }

    pub fn add_dependency(
        &mut self,
        source: TimingNodeId,
        target: TimingNodeId,
        edge: DependencyEdge,
    ) -> Result<(), Diagnostic> {
        let source_index = self.node_index(source, edge.span())?;
        let target_index = self.node_index(target, edge.span())?;
        let source_node = &self.graph[source_index];
        let target_node = &self.graph[target_index];

        let endpoints_are_valid = match edge.kind() {
            DependencyKind::Operand => matches!(
                (source_node.kind(), target_node.kind()),
                (TimingNodeKind::Signal(_), TimingNodeKind::Assignment(_))
            ),
            DependencyKind::Drive => matches!(
                (source_node.kind(), target_node.kind()),
                (
                    TimingNodeKind::Assignment(assignment),
                    TimingNodeKind::Signal(signal)
                ) if assignment.target() == signal.name()
            ),
            DependencyKind::StateBoundary => matches!(
                (source_node.kind(), target_node.kind()),
                (
                    TimingNodeKind::Assignment(assignment),
                    TimingNodeKind::Signal(signal)
                ) if assignment.target() == signal.name()
                    && signal.has_role(TimingSignalRole::ModeledRegister)
            ),
            DependencyKind::ResolvedNetBoundary => matches!(
                (source_node.kind(), target_node.kind()),
                (
                    TimingNodeKind::Assignment(assignment),
                    TimingNodeKind::Signal(signal)
                ) if assignment.target() == signal.name()
                    && signal.has_role(TimingSignalRole::ResolvedNet)
                    && !signal.has_role(TimingSignalRole::ModeledRegister)
            ),
            DependencyKind::StateControl => match (source_node.kind(), target_node.kind()) {
                (TimingNodeKind::Signal(_), TimingNodeKind::Assignment(assignment)) => self
                    .signal_ids
                    .get(assignment.target())
                    .and_then(|id| self.node_indices.get(id))
                    .is_some_and(|index| {
                        matches!(
                            self.graph[*index].kind(),
                            TimingNodeKind::Signal(signal)
                                if signal.has_role(TimingSignalRole::ModeledRegister)
                        )
                    }),
                _ => false,
            },
        };
        if !endpoints_are_valid {
            return Err(Diagnostic::error(
                edge.span().clone(),
                format!(
                    "invalid {:?} timing dependency endpoints: {source} -> {target}",
                    edge.kind()
                ),
            ));
        }

        self.graph
            .add_edge(source_index, target_index, edge.clone());
        self.dependencies.push(DependencyRecord {
            source,
            target,
            edge,
        });
        Ok(())
    }

    fn node_index(&self, id: TimingNodeId, span: &Span) -> Result<NodeIndex, Diagnostic> {
        self.node_indices
            .get(&id)
            .copied()
            .ok_or_else(|| Diagnostic::error(span.clone(), format!("unknown timing node `{id}`")))
    }

    pub fn add_constraint(
        &mut self,
        source: TimingConstraintSource,
    ) -> Result<TimingConstraintId, Diagnostic> {
        let expected_path_order = self.constraints.len();
        if source.path_order != expected_path_order {
            return Err(Diagnostic::error(
                source.span.clone(),
                format!(
                    "timing path order must be contiguous: expected {expected_path_order}, got {}",
                    source.path_order
                ),
            ));
        }
        for control in &source.controls {
            if !self.signal_ids.contains_key(control.signal()) {
                return Err(Diagnostic::error(
                    control.span().clone(),
                    format!(
                        "timing constraint control `{}` is not a known scalar signal",
                        control.signal()
                    ),
                ));
            }
        }
        if !self.signal_ids.contains_key(&source.target) {
            return Err(Diagnostic::error(
                source.target_span.clone(),
                format!(
                    "timing constraint target `{}` is not a known scalar signal",
                    source.target
                ),
            ));
        }

        let id = TimingConstraintId(self.next_constraint_id);
        self.next_constraint_id = self
            .next_constraint_id
            .checked_add(1)
            .ok_or_else(|| Diagnostic::error(source.span.clone(), "too many timing constraints"))?;
        let mut controls = Vec::with_capacity(source.controls.len());
        for (order_in_path, control_source) in source.controls.into_iter().enumerate() {
            let control_id = TimingControlId(self.next_control_id);
            self.next_control_id = self.next_control_id.checked_add(1).ok_or_else(|| {
                Diagnostic::error(source.span.clone(), "too many scalar timing controls")
            })?;
            controls.push(TimingControl {
                id: control_id,
                order_in_path,
                source: control_source,
            });
        }

        self.constraint_ids_by_path_order
            .insert(source.path_order, id);
        self.constraint_ids_by_target
            .entry(source.target.clone())
            .or_default()
            .push(id);
        self.constraints.push(TimingConstraint {
            id,
            path_order: source.path_order,
            controls,
            target: source.target,
            target_span: source.target_span,
            delay: source.delay,
            additive_delay: source.additive_delay,
            span: source.span,
        });
        Ok(id)
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &TimingNode> {
        self.node_order.iter().map(|id| {
            let index = self.node_indices[id];
            &self.graph[index]
        })
    }

    pub fn node(&self, id: TimingNodeId) -> Option<&TimingNode> {
        self.node_indices.get(&id).map(|index| &self.graph[*index])
    }

    pub fn signal_id(&self, name: &str) -> Option<TimingNodeId> {
        self.signal_ids.get(name).copied()
    }

    pub fn signal_ids(&self) -> &BTreeMap<String, TimingNodeId> {
        &self.signal_ids
    }

    pub fn assignment_id(&self, assignment_order: usize) -> Option<TimingNodeId> {
        self.assignment_ids.get(&assignment_order).copied()
    }

    pub fn dependencies(&self) -> &[DependencyRecord] {
        &self.dependencies
    }

    pub fn constraints(&self) -> &[TimingConstraint] {
        &self.constraints
    }

    pub fn target_groups(&self) -> Vec<TimingTargetGroup> {
        self.constraint_ids_by_target
            .iter()
            .map(|(target, constraint_ids)| {
                let control_ids = constraint_ids
                    .iter()
                    .flat_map(|constraint_id| {
                        self.constraints[constraint_id.ordinal() as usize]
                            .controls()
                            .iter()
                            .map(TimingControl::id)
                    })
                    .collect();
                TimingTargetGroup {
                    target: target.clone(),
                    constraint_ids: constraint_ids.clone(),
                    control_ids,
                    kind: if constraint_ids.len() == 1 {
                        TargetGroupKind::SinglePath
                    } else {
                        TargetGroupKind::MultiplePaths
                    },
                }
            })
            .collect()
    }
}

/// Builds the flat functional dependency graph without collecting specify
/// constraints. Assignment provenance must be exactly aligned with emitted
/// assignments.
pub fn build_functional_timing_graph(
    cell: &Cell,
    signal_metadata: &[TimingSignalMetadata],
    assignment_provenance: &[AssignmentProvenance],
) -> Result<TimingGraph, Diagnostic> {
    let assignments = cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    let driver_counts =
        assignments
            .iter()
            .fold(BTreeMap::<&str, usize>::new(), |mut counts, assignment| {
                *counts.entry(assignment.target.as_str()).or_default() += 1;
                counts
            });
    if assignments.len() != assignment_provenance.len() {
        let span = assignment_provenance
            .first()
            .map(|provenance| provenance.span().clone())
            .or_else(|| signal_metadata.first().map(|signal| signal.span().clone()))
            .unwrap_or_else(|| Span::new("<timing-graph>", 1, 1));
        return Err(Diagnostic::error(
            span,
            format!(
                "assignment provenance length mismatch: assignments={} provenance={}",
                assignments.len(),
                assignment_provenance.len()
            ),
        ));
    }
    cell.validate().map_err(|error| {
        let span = assignment_provenance
            .first()
            .map(|provenance| provenance.span().clone())
            .or_else(|| signal_metadata.first().map(|signal| signal.span().clone()))
            .unwrap_or_else(|| Span::new("<timing-graph>", 1, 1));
        Diagnostic::error(span, format!("invalid functional timing cell: {error}"))
    })?;

    let mut graph = TimingGraph::new();
    let mut declared_signals = signal_metadata.to_vec();
    declared_signals.sort_by(|left, right| {
        compare_spans(left.span(), right.span()).then_with(|| left.name().cmp(right.name()))
    });
    for signal in declared_signals {
        graph.add_signal(signal.name, signal.roles, signal.span)?;
    }

    // Generated temporaries are not declarations. Allocate every temporary
    // signal after declared/interface/internal signals, in emitted assignment
    // order, before Phase 1's target-resolving assignment constructor runs.
    for (order, (assignment, provenance)) in
        assignments.iter().zip(assignment_provenance).enumerate()
    {
        validate_provenance_order(order, provenance)?;
        if provenance.origin().is_temporary() && graph.signal_id(&assignment.target).is_none() {
            graph.add_signal(
                assignment.target.clone(),
                [TimingSignalRole::Internal, TimingSignalRole::Temporary]
                    .into_iter()
                    .collect(),
                provenance.span().clone(),
            )?;
        }
    }

    for (order, (assignment, provenance)) in
        assignments.iter().zip(assignment_provenance).enumerate()
    {
        validate_provenance_order(order, provenance)?;
        let (function, operands) =
            parse_flat_assignment_value(&assignment.expr, provenance.span())?;
        let assignment_id = graph.add_assignment(
            order,
            assignment.target.clone(),
            function,
            provenance.span().clone(),
        )?;

        for operand in operands {
            let Some(signal_id) = graph.signal_id(operand.atom) else {
                continue;
            };
            let Some(sense) = operand.sense else {
                continue;
            };
            graph.add_dependency(
                signal_id,
                assignment_id,
                DependencyEdge::operand(operand.index, sense, provenance.span().clone())?,
            )?;
        }

        for control in provenance.state_controls() {
            let signal = control.signal().ok_or_else(|| {
                Diagnostic::error(
                    control.span().clone(),
                    "stateful event control must be a scalar signal",
                )
            })?;
            let signal_id = graph.signal_id(signal).ok_or_else(|| {
                Diagnostic::error(
                    control.span().clone(),
                    format!("stateful event control `{signal}` is not a known scalar signal"),
                )
            })?;
            graph.add_dependency(
                signal_id,
                assignment_id,
                DependencyEdge::state_control(control.transition(), control.span().clone()),
            )?;
        }

        let target_id = graph.signal_id(&assignment.target).ok_or_else(|| {
            Diagnostic::error(
                provenance.span().clone(),
                format!(
                    "assignment target `{}` is not a known timing signal",
                    assignment.target
                ),
            )
        })?;
        let target_signal = match graph
            .node(target_id)
            .expect("assignment target was resolved to a timing node")
            .kind()
        {
            TimingNodeKind::Signal(signal) => signal,
            TimingNodeKind::Assignment(_) => {
                unreachable!("assignment targets resolve only to signal nodes")
            }
        };
        let edge = if target_signal.has_role(TimingSignalRole::ModeledRegister) {
            DependencyEdge::state_boundary(provenance.span().clone())
        } else if target_signal.has_role(TimingSignalRole::ResolvedNet)
            && driver_counts[assignment.target.as_str()] > 1
        {
            DependencyEdge::resolved_net_boundary(provenance.span().clone())
        } else {
            DependencyEdge::drive(provenance.span().clone())
        };
        graph.add_dependency(assignment_id, target_id, edge)?;
    }

    Ok(graph)
}

/// Adds every resolved specify path to an already-built functional graph.
///
/// The slice must be in exact flattened structural order. Constraint and
/// scalar-control IDs are allocated only after all invariant checks for the
/// corresponding record succeed.
pub fn collect_timing_constraints(
    graph: &mut TimingGraph,
    sources: &[TimingConstraintSource],
) -> Result<(), Diagnostic> {
    for (expected_order, source) in sources.iter().enumerate() {
        if source.path_order() != expected_order {
            return Err(Diagnostic::error(
                source.span().clone(),
                format!(
                    "timing constraint source order mismatch: expected {expected_order}, got {}",
                    source.path_order()
                ),
            ));
        }
        graph.add_constraint(source.clone())?;
    }
    Ok(())
}

/// Builds the functional graph and attaches all exact specify constraints
/// without repeating lowering or timing-expression evaluation.
pub fn build_timing_graph(
    cell: &Cell,
    signal_metadata: &[TimingSignalMetadata],
    assignment_provenance: &[AssignmentProvenance],
    constraint_sources: &[TimingConstraintSource],
) -> Result<TimingGraph, Diagnostic> {
    let mut graph = build_functional_timing_graph(cell, signal_metadata, assignment_provenance)?;
    collect_timing_constraints(&mut graph, constraint_sources)?;
    Ok(graph)
}

fn validate_provenance_order(
    expected: usize,
    provenance: &AssignmentProvenance,
) -> Result<(), Diagnostic> {
    if provenance.assignment_order() != expected {
        return Err(Diagnostic::error(
            provenance.span().clone(),
            format!(
                "assignment provenance order mismatch: expected {expected}, got {}",
                provenance.assignment_order()
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct FunctionalOperand<'a> {
    index: usize,
    atom: &'a str,
    sense: Option<TimingSense>,
}

fn parse_flat_assignment_value<'a>(
    expression: &'a Expr,
    span: &Span,
) -> Result<(AssignmentFunction, Vec<FunctionalOperand<'a>>), Diagnostic> {
    match expression {
        Expr::Atom(atom) => Ok((
            AssignmentFunction::DirectAtom,
            vec![FunctionalOperand {
                index: 0,
                atom,
                sense: Some(TimingSense::PositiveUnate),
            }],
        )),
        Expr::List(items) => {
            let Some(Expr::Atom(head)) = items.first() else {
                return Err(Diagnostic::error(
                    span.clone(),
                    "functional timing assignment operator must be a non-empty atom",
                ));
            };
            let operator = ValueOperator::parse(head).ok_or_else(|| {
                Diagnostic::error(
                    span.clone(),
                    format!("uncontracted functional timing operator `{head}`"),
                )
            })?;
            let operands = &items[1..];
            if !operator.accepts_arity(operands.len()) {
                return Err(Diagnostic::error(
                    span.clone(),
                    format!(
                        "wrong arity for functional timing operator `{}`: got {}",
                        operator.as_str(),
                        operands.len()
                    ),
                ));
            }
            let mut functional_operands = Vec::with_capacity(operands.len());
            for (index, operand) in operands.iter().enumerate() {
                let Expr::Atom(atom) = operand else {
                    return Err(Diagnostic::error(
                        span.clone(),
                        format!(
                            "functional timing operand {index} of `{}` is not flat",
                            operator.as_str()
                        ),
                    ));
                };
                functional_operands.push(FunctionalOperand {
                    index,
                    atom,
                    sense: classify_timing_sense(operator, index),
                });
            }
            Ok((AssignmentFunction::Operator(operator), functional_operands))
        }
    }
}

/// A deterministic graph view with modeled-state and multiply-driven resolved
/// net boundary edges removed into separate factual collections.
#[derive(Debug, Clone)]
pub struct CutTimingGraph {
    graph: StableDiGraph<TimingNodeId, DependencyEdge>,
    node_indices: BTreeMap<TimingNodeId, NodeIndex>,
    node_order: Vec<TimingNodeId>,
    dependencies: Vec<DependencyRecord>,
    excluded_state_boundaries: Vec<DependencyRecord>,
    excluded_resolved_net_boundaries: Vec<DependencyRecord>,
    topological_order: Vec<TimingNodeId>,
}

impl CutTimingGraph {
    pub fn nodes(&self) -> &[TimingNodeId] {
        &self.node_order
    }

    pub fn dependencies(&self) -> &[DependencyRecord] {
        &self.dependencies
    }

    pub fn excluded_state_boundaries(&self) -> &[DependencyRecord] {
        &self.excluded_state_boundaries
    }

    pub fn excluded_resolved_net_boundaries(&self) -> &[DependencyRecord] {
        &self.excluded_resolved_net_boundaries
    }

    pub fn topological_order(&self) -> &[TimingNodeId] {
        &self.topological_order
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    pub fn contains_node(&self, id: TimingNodeId) -> bool {
        self.node_indices.contains_key(&id)
    }
}

/// Removes modeled-state updates and multiply-driven resolved-net entries,
/// retaining the boundary classes separately, then rejects every remaining
/// ordinary combinational cycle.
pub fn cut_register_cycles(graph: &TimingGraph) -> Result<CutTimingGraph, Diagnostic> {
    let mut cut_graph = StableDiGraph::new();
    let mut node_indices = BTreeMap::new();
    let node_order = graph.nodes().map(TimingNode::id).collect::<Vec<_>>();
    for id in &node_order {
        node_indices.insert(*id, cut_graph.add_node(*id));
    }

    let mut dependencies = Vec::new();
    let mut excluded_state_boundaries = Vec::new();
    let mut excluded_resolved_net_boundaries = Vec::new();
    for dependency in graph.dependencies() {
        if dependency.edge().is_state_boundary() {
            excluded_state_boundaries.push(dependency.clone());
            continue;
        }
        if dependency.edge().kind() == DependencyKind::ResolvedNetBoundary {
            excluded_resolved_net_boundaries.push(dependency.clone());
            continue;
        }
        cut_graph.add_edge(
            node_indices[&dependency.source()],
            node_indices[&dependency.target()],
            dependency.edge().clone(),
        );
        dependencies.push(dependency.clone());
    }

    let mut cyclic_components = Vec::new();
    for component in kosaraju_scc(&cut_graph) {
        let is_cycle = component.len() > 1
            || component
                .first()
                .is_some_and(|index| cut_graph.find_edge(*index, *index).is_some());
        if !is_cycle {
            continue;
        }
        let mut ids = component
            .iter()
            .map(|index| cut_graph[*index])
            .collect::<Vec<_>>();
        ids.sort();
        let id_set = ids.iter().copied().collect::<BTreeSet<_>>();
        let earliest_span = dependencies
            .iter()
            .filter(|dependency| {
                id_set.contains(&dependency.source()) && id_set.contains(&dependency.target())
            })
            .map(|dependency| dependency.edge().span())
            .min_by(|left, right| compare_spans(left, right))
            .or_else(|| {
                ids.iter()
                    .filter_map(|id| graph.node(*id).map(TimingNode::span))
                    .min_by(|left, right| compare_spans(left, right))
            })
            .expect("a cyclic component contains at least one node")
            .clone();
        cyclic_components.push((earliest_span, ids));
    }
    if let Some((span, ids)) = cyclic_components
        .into_iter()
        .min_by(|left, right| compare_spans(&left.0, &right.0).then_with(|| left.1.cmp(&right.1)))
    {
        return Err(Diagnostic::error(
            span,
            format!(
                "combinational cycle remains after cutting modeled-state and multiply-driven resolved-net boundaries: {}",
                ids.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let topological_order = toposort(&cut_graph, None)
        .map_err(|cycle| {
            let id = cut_graph[cycle.node_id()];
            let span = graph
                .node(id)
                .map(TimingNode::span)
                .cloned()
                .unwrap_or_else(|| Span::new("<timing-graph>", 1, 1));
            Diagnostic::error(
                span,
                format!("combinational cycle remains at timing node `{id}`"),
            )
        })?
        .into_iter()
        .map(|index| cut_graph[index])
        .collect();

    Ok(CutTimingGraph {
        graph: cut_graph,
        node_indices,
        node_order,
        dependencies,
        excluded_state_boundaries,
        excluded_resolved_net_boundaries,
        topological_order,
    })
}

/// Verifies every scalar specify control against the full functional graph.
/// State and resolved-net boundary edges are deliberately present here: they
/// are cut only for combinational-cycle validation, never for path
/// reachability.
pub fn validate_constraint_reachability(graph: &TimingGraph) -> Result<(), Diagnostic> {
    for constraint in graph.constraints() {
        let target = graph.signal_ids[constraint.target()];
        let target_index = graph.node_indices[&target];
        for control in constraint.controls() {
            let source = graph.signal_ids[control.source().signal()];
            let source_index = graph.node_indices[&source];
            if !has_path_connecting(&graph.graph, source_index, target_index, None) {
                return Err(Diagnostic::error(
                    control.source().span().clone(),
                    format!(
                        "timing constraint {} control {} `{}` cannot reach target `{}` in the full functional graph",
                        constraint.id(),
                        control.id(),
                        control.source().signal(),
                        constraint.target()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Computes deterministic reachability, dominance, post-dominance,
/// reconvergence, path-sense, and public-output classifications.
pub fn analyze_timing_graph(
    graph: &TimingGraph,
    cut_graph: &CutTimingGraph,
) -> Result<TimingAnalysisReport, Diagnostic> {
    validate_constraint_reachability(graph)?;
    let control_groups = analyze_control_groups(graph);
    let mut target_groups = Vec::new();
    for group in graph.target_groups() {
        target_groups.push(analyze_target_group(graph, group)?);
    }
    Ok(TimingAnalysisReport {
        nodes: graph.nodes().cloned().collect(),
        dependencies: graph.dependencies().to_vec(),
        cut_dependencies: cut_graph.dependencies().to_vec(),
        excluded_state_boundaries: cut_graph.excluded_state_boundaries().to_vec(),
        excluded_resolved_net_boundaries: cut_graph.excluded_resolved_net_boundaries().to_vec(),
        cut_topological_order: cut_graph.topological_order().to_vec(),
        constraints: graph.constraints().to_vec(),
        control_groups,
        target_groups,
    })
}

fn analyze_control_groups(graph: &TimingGraph) -> Vec<TimingControlGroupReport> {
    let mut records_by_signal =
        BTreeMap::<String, Vec<(TimingConstraintId, TimingControlId, TimingNodeId)>>::new();
    for constraint in graph.constraints() {
        let target_node = graph.signal_ids[constraint.target()];
        for control in constraint.controls() {
            records_by_signal
                .entry(control.source().signal().to_string())
                .or_default()
                .push((constraint.id(), control.id(), target_node));
        }
    }

    records_by_signal
        .into_iter()
        .map(|(control_signal, records)| {
            let source_node = graph.signal_ids[&control_signal];
            let source_index = graph.node_indices[&source_node];
            let forward_dominators = simple_fast(&graph.graph, source_index);

            let mut seen_constraints = BTreeSet::new();
            let constraint_ids = records
                .iter()
                .map(|(constraint_id, _, _)| *constraint_id)
                .filter(|constraint_id| seen_constraints.insert(*constraint_id))
                .collect();
            let control_ids = records
                .iter()
                .map(|(_, control_id, _)| *control_id)
                .collect();
            let target_set = records
                .iter()
                .map(|(_, _, target_node)| *target_node)
                .collect::<BTreeSet<_>>();
            let target_nodes = ids_in_graph_order(graph, &target_set);
            let mut common_prefix = intersect_node_sets(target_nodes.iter().map(|target_node| {
                let target_index = graph.node_indices[target_node];
                forward_dominators
                    .dominators(target_index)
                    .expect("reachability validation precedes dominance analysis")
                    .map(|index| graph.graph[index].id())
                    .collect()
            }));
            common_prefix.remove(&source_node);
            for target_node in &target_nodes {
                common_prefix.remove(target_node);
            }

            TimingControlGroupReport {
                control_signal,
                source_node,
                constraint_ids,
                control_ids,
                kind: if target_nodes.len() == 1 {
                    ControlGroupKind::SingleTarget
                } else {
                    ControlGroupKind::MultipleTargets
                },
                target_nodes,
                common_prefix: ids_in_graph_order(graph, &common_prefix),
            }
        })
        .collect()
}

fn analyze_target_group(
    graph: &TimingGraph,
    group: TimingTargetGroup,
) -> Result<TimingTargetGroupReport, Diagnostic> {
    let target_node = graph.signal_ids[&group.target];
    let target_index = graph.node_indices[&target_node];
    let reverse_dominators = simple_fast(Reversed(&graph.graph), target_index);
    let mut control_reports = Vec::new();

    for constraint_id in &group.constraint_ids {
        let constraint = &graph.constraints[constraint_id.ordinal() as usize];
        for control in constraint.controls() {
            let source_node = graph.signal_ids[control.source().signal()];
            let source_index = graph.node_indices[&source_node];
            let forward_dominators = simple_fast(&graph.graph, source_index);
            let target_dominators = forward_dominators
                .dominators(target_index)
                .expect("reachability validation precedes dominance analysis")
                .map(|index| graph.graph[index].id())
                .collect::<BTreeSet<_>>();
            let target_post_dominators = reverse_dominators
                .dominators(source_index)
                .expect("reachability validation precedes post-dominance analysis")
                .map(|index| graph.graph[index].id())
                .collect::<BTreeSet<_>>();
            let reachable_nodes = control_to_target_slice(graph, source_node, target_node);
            let path_senses =
                compose_path_senses(graph, source_node, target_node, &reachable_nodes);
            control_reports.push(TimingControlReport {
                constraint_id: *constraint_id,
                control_id: control.id(),
                source_node,
                target_node,
                reachable_nodes,
                target_dominators: ids_in_graph_order(graph, &target_dominators),
                target_post_dominators: ids_in_graph_order(graph, &target_post_dominators),
                path_senses,
            });
        }
    }

    let control_nodes = control_reports
        .iter()
        .map(TimingControlReport::source_node)
        .collect::<BTreeSet<_>>();
    let distinct_source_reports = control_reports
        .iter()
        .fold(
            BTreeMap::<TimingNodeId, &TimingControlReport>::new(),
            |mut reports, report| {
                reports.entry(report.source_node()).or_insert(report);
                reports
            },
        )
        .into_values();
    let mut common_suffix = intersect_node_sets(
        distinct_source_reports
            .map(|report| report.target_post_dominators().iter().copied().collect()),
    );
    common_suffix.remove(&target_node);
    for source_node in &control_nodes {
        common_suffix.remove(source_node);
    }
    let slice = control_reports
        .iter()
        .flat_map(|report| report.reachable_nodes().iter().copied())
        .collect::<BTreeSet<_>>();

    // A reconvergent node has at least two distinct predecessor nodes within
    // the union of control-to-target slices. Parallel/repeated operand edges
    // from one predecessor therefore remain visible in dependencies but count
    // as exactly one incoming branch for this classification.
    let reconvergent_nodes = graph
        .node_order
        .iter()
        .copied()
        .filter(|target| slice.contains(target))
        .filter(|target| {
            graph
                .dependencies
                .iter()
                .filter(|dependency| dependency.target() == *target)
                .filter(|dependency| slice.contains(&dependency.source()))
                .map(DependencyRecord::source)
                .collect::<BTreeSet<_>>()
                .len()
                >= 2
        })
        .collect();

    let public_output_split = classify_public_output_split(graph, target_node);
    Ok(TimingTargetGroupReport {
        reachable_controls: control_reports
            .iter()
            .map(TimingControlReport::control_id)
            .collect(),
        control_reports,
        common_suffix: ids_in_graph_order(graph, &common_suffix),
        reconvergent_nodes,
        public_output_split,
        group,
    })
}

fn control_to_target_slice(
    graph: &TimingGraph,
    source: TimingNodeId,
    target: TimingNodeId,
) -> Vec<TimingNodeId> {
    let source_index = graph.node_indices[&source];
    let target_index = graph.node_indices[&target];
    graph
        .node_order
        .iter()
        .copied()
        .filter(|id| {
            let index = graph.node_indices[id];
            has_path_connecting(&graph.graph, source_index, index, None)
                && has_path_connecting(&graph.graph, index, target_index, None)
        })
        .collect()
}

fn intersect_node_sets(
    sets: impl IntoIterator<Item = BTreeSet<TimingNodeId>>,
) -> BTreeSet<TimingNodeId> {
    let mut sets = sets.into_iter();
    let Some(mut common) = sets.next() else {
        return BTreeSet::new();
    };
    for nodes in sets {
        common.retain(|node| nodes.contains(node));
    }
    common
}

fn ids_in_graph_order(graph: &TimingGraph, ids: &BTreeSet<TimingNodeId>) -> Vec<TimingNodeId> {
    graph
        .node_order
        .iter()
        .copied()
        .filter(|id| ids.contains(id))
        .collect()
}

fn compose_path_senses(
    graph: &TimingGraph,
    source: TimingNodeId,
    target: TimingNodeId,
    slice: &[TimingNodeId],
) -> Vec<TimingPathSense> {
    let slice = slice.iter().copied().collect::<BTreeSet<_>>();
    let mut senses = graph
        .node_order
        .iter()
        .copied()
        .map(|id| (id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    senses
        .get_mut(&source)
        .expect("source is a durable graph node")
        .insert(TimingPathSense::PositiveUnate);

    loop {
        let mut changed = false;
        for dependency in &graph.dependencies {
            if !slice.contains(&dependency.source()) || !slice.contains(&dependency.target()) {
                continue;
            }
            let incoming = senses[&dependency.source()].clone();
            for sense in incoming {
                let composed = compose_dependency_sense(sense, dependency.edge());
                changed |= senses
                    .get_mut(&dependency.target())
                    .expect("dependency target is a durable graph node")
                    .insert(composed);
            }
        }
        if !changed {
            break;
        }
    }
    senses
        .remove(&target)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn compose_dependency_sense(current: TimingPathSense, edge: &DependencyEdge) -> TimingPathSense {
    if edge.kind() == DependencyKind::StateControl {
        let event_transition = edge.event_transition();
        return TimingPathSense::StateControl {
            event_transition,
            target_effect: event_transition.map(TransitionEffect::Exact),
        };
    }

    match current {
        TimingPathSense::StateControl {
            event_transition,
            target_effect,
        } => TimingPathSense::StateControl {
            event_transition,
            target_effect: target_effect.map(|effect| match effect {
                TransitionEffect::Exact(transition) => {
                    propagate_transition(edge.sense(), transition)
                }
                TransitionEffect::Indeterminate => TransitionEffect::Indeterminate,
            }),
        },
        TimingPathSense::NonUnate => TimingPathSense::NonUnate,
        TimingPathSense::Conditional => match edge.sense() {
            TimingSense::NonUnate => TimingPathSense::NonUnate,
            TimingSense::PositiveUnate | TimingSense::NegativeUnate | TimingSense::Conditional => {
                TimingPathSense::Conditional
            }
            TimingSense::StateControl => unreachable!("state controls are handled by edge kind"),
        },
        TimingPathSense::PositiveUnate => match edge.sense() {
            TimingSense::PositiveUnate => TimingPathSense::PositiveUnate,
            TimingSense::NegativeUnate => TimingPathSense::NegativeUnate,
            TimingSense::NonUnate => TimingPathSense::NonUnate,
            TimingSense::Conditional => TimingPathSense::Conditional,
            TimingSense::StateControl => unreachable!("state controls are handled by edge kind"),
        },
        TimingPathSense::NegativeUnate => match edge.sense() {
            TimingSense::PositiveUnate => TimingPathSense::NegativeUnate,
            TimingSense::NegativeUnate => TimingPathSense::PositiveUnate,
            TimingSense::NonUnate => TimingPathSense::NonUnate,
            TimingSense::Conditional => TimingPathSense::Conditional,
            TimingSense::StateControl => unreachable!("state controls are handled by edge kind"),
        },
    }
}

fn classify_public_output_split(graph: &TimingGraph, target: TimingNodeId) -> PublicOutputSplit {
    let is_public = graph.node(target).is_some_and(|node| match node.kind() {
        TimingNodeKind::Signal(signal) => {
            signal.has_role(TimingSignalRole::Output) || signal.has_role(TimingSignalRole::Inout)
        }
        TimingNodeKind::Assignment(_) => false,
    });
    if !is_public {
        return PublicOutputSplit::NotPublic;
    }
    if graph.dependencies.iter().any(|dependency| {
        dependency.source() == target && dependency.edge().kind() == DependencyKind::Operand
    }) {
        PublicOutputSplit::Candidate
    } else {
        PublicOutputSplit::NotRequired
    }
}

fn render_timing_analysis_report(report: &TimingAnalysisReport) -> String {
    use std::fmt::Write as _;

    let mut output = String::from("timing-analysis\n");
    for node in &report.nodes {
        let kind = match node.kind() {
            TimingNodeKind::Signal(signal) => format!(
                "signal {} roles={}",
                signal.name(),
                signal
                    .roles()
                    .iter()
                    .map(|role| timing_signal_role_name(*role))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            TimingNodeKind::Assignment(assignment) => format!(
                "assignment order={} target={} function={}",
                assignment.assignment_order(),
                assignment.target(),
                match assignment.function() {
                    AssignmentFunction::DirectAtom => "direct",
                    AssignmentFunction::Operator(operator) => operator.as_str(),
                }
            ),
        };
        writeln!(
            output,
            "node {} {kind} @{}",
            node.id(),
            render_span(node.span())
        )
        .unwrap();
    }
    for dependency in &report.dependencies {
        writeln!(
            output,
            "dependency {} -> {} kind={} operand={} sense={} event={} @{}",
            dependency.source(),
            dependency.target(),
            dependency_kind_name(dependency.edge().kind()),
            dependency
                .edge()
                .operand_index()
                .map_or_else(|| "-".to_string(), |index| index.to_string()),
            timing_sense_name(dependency.edge().sense()),
            dependency
                .edge()
                .event_transition()
                .map_or("-", transition_name),
            render_span(dependency.edge().span())
        )
        .unwrap();
    }
    writeln!(
        output,
        "cut-order {}",
        render_ids(&report.cut_topological_order)
    )
    .unwrap();
    for dependency in &report.cut_dependencies {
        writeln!(
            output,
            "cut-dependency {} -> {} kind={}",
            dependency.source(),
            dependency.target(),
            dependency_kind_name(dependency.edge().kind())
        )
        .unwrap();
    }
    for dependency in &report.excluded_state_boundaries {
        writeln!(
            output,
            "cut-excluded {} -> {} kind=state-boundary",
            dependency.source(),
            dependency.target()
        )
        .unwrap();
    }
    for dependency in &report.excluded_resolved_net_boundaries {
        writeln!(
            output,
            "cut-excluded {} -> {} kind=resolved-net-boundary",
            dependency.source(),
            dependency.target()
        )
        .unwrap();
    }
    for constraint in &report.constraints {
        writeln!(
            output,
            "constraint {} order={} target={} target@{} path@{} delay={}",
            constraint.id(),
            constraint.path_order(),
            constraint.target(),
            render_span(constraint.target_span()),
            render_span(constraint.span()),
            crate::serialize::render_delay_tuple(constraint.delay())
        )
        .unwrap();
        for (component, additive) in constraint.additive_delay().components().enumerate() {
            writeln!(
                output,
                "  terms[{component}]={}",
                additive
                    .terms()
                    .iter()
                    .map(|term| crate::serialize::render_timing_expr(term.as_timing_expr()))
                    .collect::<Vec<_>>()
                    .join(" + ")
            )
            .unwrap();
        }
        for control in constraint.controls() {
            writeln!(
                output,
                "  control {} order={} signal={} event={} @{}",
                control.id(),
                control.order_in_path(),
                control.source().signal(),
                control.source().transition().map_or("-", transition_name),
                render_span(control.source().span())
            )
            .unwrap();
        }
    }
    for group in &report.control_groups {
        writeln!(
            output,
            "control-group signal={} source={} kind={} constraints={} controls={} targets={} prefix={}",
            group.control_signal(),
            group.source_node(),
            match group.kind() {
                ControlGroupKind::SingleTarget => "single-target",
                ControlGroupKind::MultipleTargets => "multiple-targets",
            },
            render_ids(group.constraint_ids()),
            render_ids(group.control_ids()),
            render_ids(group.target_nodes()),
            render_ids(group.common_prefix()),
        )
        .unwrap();
    }
    for group in &report.target_groups {
        writeln!(
            output,
            "group target={} kind={} constraints={} controls={} suffix={} reconvergent={} split={}",
            group.group().target(),
            match group.group().kind() {
                TargetGroupKind::SinglePath => "single",
                TargetGroupKind::MultiplePaths => "multiple",
            },
            render_ids(group.group().constraint_ids()),
            render_ids(group.reachable_controls()),
            render_ids(group.common_suffix()),
            render_ids(group.reconvergent_nodes()),
            match group.public_output_split() {
                PublicOutputSplit::NotPublic => "not-public",
                PublicOutputSplit::NotRequired => "not-required",
                PublicOutputSplit::Candidate => "candidate",
            }
        )
        .unwrap();
        for control in group.control_reports() {
            writeln!(
                output,
                "  control-report {} constraint={} source={} target={} slice={} dominators={} post-dominators={} senses={}",
                control.control_id(),
                control.constraint_id(),
                control.source_node(),
                control.target_node(),
                render_ids(control.reachable_nodes()),
                render_ids(control.target_dominators()),
                render_ids(control.target_post_dominators()),
                control
                    .path_senses()
                    .iter()
                    .map(render_path_sense)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
        }
    }
    output
}

fn render_ids<T: fmt::Display>(ids: &[T]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn render_path_sense(sense: &TimingPathSense) -> String {
    match sense {
        TimingPathSense::PositiveUnate => "positive".to_string(),
        TimingPathSense::NegativeUnate => "negative".to_string(),
        TimingPathSense::NonUnate => "non-unate".to_string(),
        TimingPathSense::Conditional => "conditional".to_string(),
        TimingPathSense::StateControl {
            event_transition,
            target_effect,
        } => format!(
            "state-control({}->{})",
            event_transition.map_or("level", transition_name),
            match target_effect {
                Some(TransitionEffect::Exact(transition)) => transition_name(*transition),
                Some(TransitionEffect::Indeterminate) => "indeterminate",
                None => "level",
            }
        ),
    }
}

const fn timing_signal_role_name(role: TimingSignalRole) -> &'static str {
    match role {
        TimingSignalRole::Input => "input",
        TimingSignalRole::Output => "output",
        TimingSignalRole::Inout => "inout",
        TimingSignalRole::ResolvedNet => "resolved-net",
        TimingSignalRole::ModeledRegister => "register",
        TimingSignalRole::Internal => "internal",
        TimingSignalRole::Temporary => "temporary",
        TimingSignalRole::TimingTemporary => "timing-temporary",
        TimingSignalRole::TopologyTemporary => "topology-temporary",
    }
}

const fn dependency_kind_name(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Operand => "operand",
        DependencyKind::Drive => "drive",
        DependencyKind::StateBoundary => "state-boundary",
        DependencyKind::ResolvedNetBoundary => "resolved-net-boundary",
        DependencyKind::StateControl => "state-control",
    }
}

const fn timing_sense_name(sense: TimingSense) -> &'static str {
    match sense {
        TimingSense::PositiveUnate => "positive",
        TimingSense::NegativeUnate => "negative",
        TimingSense::NonUnate => "non-unate",
        TimingSense::Conditional => "conditional",
        TimingSense::StateControl => "state-control",
    }
}

const fn transition_name(transition: Transition) -> &'static str {
    match transition {
        Transition::Rise => "rise",
        Transition::Fall => "fall",
        Transition::TurnOff => "turn-off",
    }
}

fn render_span(span: &Span) -> String {
    let path = if span.path.is_absolute() {
        span.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| normalized_path(&span.path))
    } else {
        normalized_path(&span.path)
    };
    format!("{path}:{}:{}", span.line, span.column)
}

fn compare_spans(left: &Span, right: &Span) -> std::cmp::Ordering {
    normalized_path(&left.path)
        .cmp(&normalized_path(&right.path))
        .then_with(|| left.line.cmp(&right.line))
        .then_with(|| left.column.cmp(&right.column))
}

fn normalized_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Assignment, LogicValue, Register, TimingExpr};

    fn span(line: usize) -> Span {
        Span::new("graph.sv", line, 1)
    }

    fn roles(roles: &[TimingSignalRole]) -> BTreeSet<TimingSignalRole> {
        roles.iter().copied().collect()
    }

    fn delay(value: &str) -> DelayTuple {
        DelayTuple::One(TimingExpr::atom(value).unwrap())
    }

    fn control(signal: &str, line: usize) -> TimingControlSource {
        TimingControlSource::new(signal, None, span(line)).unwrap()
    }

    fn signal_metadata(
        name: &str,
        roles: &[TimingSignalRole],
        line: usize,
    ) -> TimingSignalMetadata {
        TimingSignalMetadata::new(
            name.to_string(),
            roles.iter().copied().collect(),
            span(line),
        )
        .unwrap()
    }

    fn assignment_provenance(
        order: usize,
        line: usize,
        origin: SourceAssignmentOrigin,
        controls: Vec<StateControlProvenance>,
    ) -> AssignmentProvenance {
        AssignmentProvenance::new(
            order,
            order,
            span(line),
            AssignmentOrigin::Source(origin),
            controls,
        )
        .unwrap()
    }

    fn functional_cell(target: &str, expression: Expr, register: bool) -> Cell {
        Cell {
            name: "functional".to_string(),
            inputs: Vec::new(),
            outputs: vec![target.to_string()],
            registers: register
                .then(|| Register {
                    name: target.to_string(),
                    initial: LogicValue::X,
                })
                .into_iter()
                .collect(),
            items: vec![CellItem::Assignment(Assignment {
                target: target.to_string(),
                expr: expression,
                delay: delay("0"),
            })],
        }
    }

    fn resolved_cycle_cell(expressions: Vec<Expr>, register: bool) -> Cell {
        Cell {
            name: "resolved_cycle".to_string(),
            inputs: vec!["a".to_string()],
            outputs: vec!["r".to_string()],
            registers: register
                .then(|| Register {
                    name: "r".to_string(),
                    initial: LogicValue::X,
                })
                .into_iter()
                .collect(),
            items: expressions
                .into_iter()
                .map(|expr| {
                    CellItem::Assignment(Assignment {
                        target: "r".to_string(),
                        expr,
                        delay: delay("0"),
                    })
                })
                .collect(),
        }
    }

    fn add_test_assignment(
        graph: &mut TimingGraph,
        order: usize,
        target: &str,
        function: AssignmentFunction,
        operands: &[(TimingNodeId, usize, TimingSense)],
        line: usize,
    ) -> TimingNodeId {
        let assignment = graph
            .add_assignment(order, target, function, span(line))
            .unwrap();
        for (source, position, sense) in operands {
            graph
                .add_dependency(
                    *source,
                    assignment,
                    DependencyEdge::operand(*position, *sense, span(line)).unwrap(),
                )
                .unwrap();
        }
        let target_node = graph.signal_id(target).unwrap();
        graph
            .add_dependency(assignment, target_node, DependencyEdge::drive(span(line)))
            .unwrap();
        assignment
    }

    #[test]
    fn allocates_durable_ids_and_retains_source_order_separately_from_name_order() {
        let mut graph = TimingGraph::new();
        let z = graph
            .add_signal("z", roles(&[TimingSignalRole::Input]), span(1))
            .unwrap();
        let a = graph
            .add_signal("a", roles(&[TimingSignalRole::Internal]), span(2))
            .unwrap();
        let assignment = graph
            .add_assignment(
                0,
                "a",
                AssignmentFunction::Operator(ValueOperator::Not),
                span(3),
            )
            .unwrap();

        assert_eq!(
            (z.to_string(), a.to_string(), assignment.to_string()),
            ("n0".into(), "n1".into(), "n2".into())
        );
        assert_eq!(
            graph.nodes().map(TimingNode::id).collect::<Vec<_>>(),
            vec![z, a, assignment]
        );
        assert_eq!(
            graph
                .signal_ids()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert_eq!(graph.assignment_id(0), Some(assignment));
        assert_eq!(
            match graph.node(assignment).unwrap().kind() {
                TimingNodeKind::Assignment(assignment) => assignment.function(),
                TimingNodeKind::Signal(_) => unreachable!(),
            },
            AssignmentFunction::Operator(ValueOperator::Not)
        );
        assert_eq!(graph.nodes().len(), 3);
    }

    #[test]
    fn dependencies_preserve_operand_position_sense_state_boundaries_and_order() {
        let mut graph = TimingGraph::new();
        let input = graph
            .add_signal("d", roles(&[TimingSignalRole::Input]), span(1))
            .unwrap();
        let register = graph
            .add_signal(
                "q",
                roles(&[TimingSignalRole::Output, TimingSignalRole::ModeledRegister]),
                span(2),
            )
            .unwrap();
        let assignment = graph
            .add_assignment(0, "q", AssignmentFunction::DirectAtom, span(3))
            .unwrap();

        graph
            .add_dependency(
                input,
                assignment,
                DependencyEdge::operand(0, TimingSense::PositiveUnate, span(4)).unwrap(),
            )
            .unwrap();
        graph
            .add_dependency(
                input,
                assignment,
                DependencyEdge::operand(1, TimingSense::NegativeUnate, span(4)).unwrap(),
            )
            .unwrap();
        graph
            .add_dependency(
                input,
                assignment,
                DependencyEdge::state_control(None, span(5)),
            )
            .unwrap();
        graph
            .add_dependency(
                assignment,
                register,
                DependencyEdge::state_boundary(span(6)),
            )
            .unwrap();

        assert_eq!(graph.dependencies().len(), 4);
        assert_eq!(graph.dependencies()[0].source(), input);
        assert_eq!(graph.dependencies()[0].target(), assignment);
        assert_eq!(graph.dependencies()[0].edge().operand_index(), Some(0));
        assert_eq!(
            graph.dependencies()[1].edge().sense(),
            TimingSense::NegativeUnate
        );
        assert_eq!(
            graph.dependencies()[2].edge().kind(),
            DependencyKind::StateControl
        );
        assert!(graph.dependencies()[3].edge().is_state_boundary());
    }

    #[test]
    fn dependency_validation_rejects_invalid_metadata_and_endpoint_shapes() {
        assert!(DependencyEdge::operand(0, TimingSense::StateControl, span(1)).is_err());
        assert!(
            DependencyEdge::try_new(
                DependencyKind::Drive,
                Some(0),
                TimingSense::PositiveUnate,
                None,
                span(2)
            )
            .is_err()
        );

        let mut graph = TimingGraph::new();
        let input = graph
            .add_signal("d", roles(&[TimingSignalRole::Input]), span(3))
            .unwrap();
        let output = graph
            .add_signal("q", roles(&[TimingSignalRole::Output]), span(4))
            .unwrap();
        let other_output = graph
            .add_signal("r", roles(&[TimingSignalRole::Output]), span(5))
            .unwrap();
        let assignment = graph
            .add_assignment(0, "q", AssignmentFunction::DirectAtom, span(6))
            .unwrap();

        assert!(
            graph
                .add_dependency(input, output, DependencyEdge::drive(span(7)))
                .is_err()
        );
        assert!(
            graph
                .add_dependency(assignment, other_output, DependencyEdge::drive(span(8)))
                .is_err(),
            "a drive cannot contradict the assignment's named target"
        );
        assert!(
            graph
                .add_dependency(assignment, output, DependencyEdge::state_boundary(span(9)))
                .is_err(),
            "an ordinary output is not a modeled state boundary"
        );
        assert!(
            graph
                .add_dependency(
                    input,
                    assignment,
                    DependencyEdge::state_control(Some(Transition::Rise), span(10))
                )
                .is_err(),
            "state controls apply only to assignments updating modeled registers"
        );

        let register = graph
            .add_signal(
                "state_q",
                roles(&[TimingSignalRole::ModeledRegister]),
                span(11),
            )
            .unwrap();
        let other_register = graph
            .add_signal(
                "state_r",
                roles(&[TimingSignalRole::ModeledRegister]),
                span(12),
            )
            .unwrap();
        let state_assignment = graph
            .add_assignment(1, "state_q", AssignmentFunction::DirectAtom, span(13))
            .unwrap();
        assert!(
            graph
                .add_dependency(
                    state_assignment,
                    other_register,
                    DependencyEdge::state_boundary(span(14))
                )
                .is_err()
        );
        graph
            .add_dependency(
                state_assignment,
                register,
                DependencyEdge::state_boundary(span(15)),
            )
            .unwrap();
    }

    #[test]
    fn signal_and_assignment_constructors_enforce_unique_valid_identity() {
        let mut graph = TimingGraph::new();
        assert!(
            graph
                .add_signal("", roles(&[TimingSignalRole::Input]), span(1))
                .is_err()
        );
        assert!(
            graph
                .add_signal("empty_roles", BTreeSet::new(), span(2))
                .is_err()
        );
        assert!(
            graph
                .add_assignment(0, "unknown", AssignmentFunction::DirectAtom, span(3))
                .is_err(),
            "every assignment target must resolve before the assignment is added"
        );
        graph
            .add_signal("a", roles(&[TimingSignalRole::Input]), span(4))
            .unwrap();
        assert!(
            graph
                .add_signal("a", roles(&[TimingSignalRole::Output]), span(5))
                .is_err()
        );
        graph
            .add_assignment(7, "a", AssignmentFunction::DirectAtom, span(6))
            .unwrap();
        assert!(
            graph
                .add_assignment(7, "a", AssignmentFunction::DirectAtom, span(7))
                .is_err()
        );
        assert!(
            graph
                .add_assignment(8, "", AssignmentFunction::DirectAtom, span(8))
                .is_err()
        );
    }

    #[test]
    fn constraints_allocate_source_ordered_path_and_control_ids_and_sorted_groups() {
        let mut graph = TimingGraph::new();
        for (name, role) in [
            ("z0", TimingSignalRole::Input),
            ("z1", TimingSignalRole::Input),
            ("z", TimingSignalRole::Output),
            ("a0", TimingSignalRole::Input),
            ("a", TimingSignalRole::Output),
            ("z2", TimingSignalRole::Input),
        ] {
            graph.add_signal(name, roles(&[role]), span(1)).unwrap();
        }
        let z_path = graph
            .add_constraint(
                TimingConstraintSource::new(
                    0,
                    vec![control("z0", 1), control("z1", 2)],
                    "z",
                    delay("T_z0"),
                    span(1),
                )
                .unwrap(),
            )
            .unwrap();
        let a_path = graph
            .add_constraint(
                TimingConstraintSource::new(1, vec![control("a0", 3)], "a", delay("T_a"), span(3))
                    .unwrap(),
            )
            .unwrap();
        let z_path_2 = graph
            .add_constraint(
                TimingConstraintSource::new(2, vec![control("z2", 4)], "z", delay("T_z2"), span(4))
                    .unwrap(),
            )
            .unwrap();

        assert_eq!(
            (z_path.to_string(), a_path.to_string(), z_path_2.to_string()),
            ("p0".into(), "p1".into(), "p2".into())
        );
        assert_eq!(
            graph
                .constraints()
                .iter()
                .flat_map(TimingConstraint::controls)
                .map(TimingControl::id)
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
            vec!["c0", "c1", "c2", "c3"]
        );

        let groups = graph.target_groups();
        assert_eq!(
            groups
                .iter()
                .map(TimingTargetGroup::target)
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert_eq!(groups[0].kind(), TargetGroupKind::SinglePath);
        assert_eq!(groups[0].constraint_ids(), &[a_path]);
        assert_eq!(groups[1].kind(), TargetGroupKind::MultiplePaths);
        assert_eq!(groups[1].constraint_ids(), &[z_path, z_path_2]);
        assert_eq!(
            groups[1]
                .control_ids()
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
            vec!["c0", "c1", "c3"]
        );
    }

    #[test]
    fn constraint_sources_validate_controls_targets_delays_and_unique_path_order() {
        assert!(
            TimingControlSource::new("", None, span(1)).is_err(),
            "empty scalar control"
        );
        assert!(
            TimingConstraintSource::new(0, Vec::new(), "q", delay("T"), span(2)).is_err(),
            "constraint without scalar controls"
        );
        assert!(
            TimingConstraintSource::new(0, vec![control("clk", 3)], "", delay("T"), span(3))
                .is_err(),
            "empty scalar target"
        );

        let mut graph = TimingGraph::new();
        graph
            .add_signal("clk", roles(&[TimingSignalRole::Input]), span(4))
            .unwrap();
        graph
            .add_signal("reset", roles(&[TimingSignalRole::Input]), span(5))
            .unwrap();
        graph
            .add_signal("q", roles(&[TimingSignalRole::Output]), span(4))
            .unwrap();
        graph
            .add_constraint(
                TimingConstraintSource::new(0, vec![control("clk", 4)], "q", delay("T"), span(4))
                    .unwrap(),
            )
            .unwrap();
        assert!(
            graph
                .add_constraint(
                    TimingConstraintSource::new(
                        0,
                        vec![control("reset", 5)],
                        "q",
                        delay("T2"),
                        span(5),
                    )
                    .unwrap()
                )
                .is_err(),
            "path order is a durable source identity"
        );
    }

    #[test]
    fn all_contracted_operators_have_exact_functional_operand_sense() {
        let cases = [
            (ValueOperator::Not, vec![(0, TimingSense::NegativeUnate)]),
            (
                ValueOperator::And,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::PositiveUnate),
                ],
            ),
            (
                ValueOperator::Or,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::PositiveUnate),
                ],
            ),
            (
                ValueOperator::Xor,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (
                ValueOperator::Nand,
                vec![
                    (0, TimingSense::NegativeUnate),
                    (1, TimingSense::NegativeUnate),
                ],
            ),
            (
                ValueOperator::Nor,
                vec![
                    (0, TimingSense::NegativeUnate),
                    (1, TimingSense::NegativeUnate),
                ],
            ),
            (
                ValueOperator::Xnor,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (
                ValueOperator::Mux,
                vec![
                    (0, TimingSense::NonUnate),
                    (1, TimingSense::Conditional),
                    (2, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::BufIf0,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::BufIf1,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::DriveStrength,
                vec![(0, TimingSense::PositiveUnate)],
            ),
            (
                ValueOperator::BufIf0Strength,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::BufIf1Strength,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::Eq,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (
                ValueOperator::CaseEq,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (
                ValueOperator::Neq,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (
                ValueOperator::CaseNeq,
                vec![(0, TimingSense::NonUnate), (1, TimingSense::NonUnate)],
            ),
            (ValueOperator::Keeper, vec![]),
            (
                ValueOperator::Nmos,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::Pmos,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
            (
                ValueOperator::Rnmos,
                vec![
                    (0, TimingSense::PositiveUnate),
                    (1, TimingSense::Conditional),
                ],
            ),
        ];
        assert_eq!(cases.len(), ValueOperator::ALL.len());

        for (operator, expected) in cases {
            let operand_names = match operator {
                ValueOperator::Not => vec!["a"],
                ValueOperator::Mux => vec!["a", "b", "c"],
                ValueOperator::DriveStrength => vec!["a", "strong1", "highz0"],
                ValueOperator::BufIf0Strength => {
                    vec!["a", "b", "strong1", "highz0"]
                }
                ValueOperator::BufIf1Strength => {
                    vec!["a", "b", "highz1", "strong0"]
                }
                ValueOperator::Keeper => Vec::new(),
                _ => vec!["a", "b"],
            };
            let expression = Expr::value(
                operator,
                operand_names.iter().map(|name| Expr::atom(*name)).collect(),
            );
            let cell = functional_cell("y", expression, false);
            // Strength tokens are deliberately also present as known signals:
            // operator-position rules, not name matching, must exclude them.
            let metadata = ["y", "a", "b", "c", "strong1", "highz0", "highz1", "strong0"]
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    signal_metadata(
                        name,
                        if *name == "y" {
                            &[TimingSignalRole::Output]
                        } else {
                            &[TimingSignalRole::Input]
                        },
                        index + 1,
                    )
                })
                .collect::<Vec<_>>();
            let provenance = vec![assignment_provenance(
                0,
                20,
                if operator == ValueOperator::Keeper {
                    SourceAssignmentOrigin::Keeper
                } else if matches!(
                    operator,
                    ValueOperator::BufIf0
                        | ValueOperator::BufIf1
                        | ValueOperator::BufIf0Strength
                        | ValueOperator::BufIf1Strength
                        | ValueOperator::Nmos
                        | ValueOperator::Pmos
                        | ValueOperator::Rnmos
                ) {
                    SourceAssignmentOrigin::Primitive
                } else {
                    SourceAssignmentOrigin::Continuous
                },
                Vec::new(),
            )];
            let graph = build_functional_timing_graph(&cell, &metadata, &provenance).unwrap();
            let actual = graph
                .dependencies()
                .iter()
                .filter(|dependency| dependency.edge().kind() == DependencyKind::Operand)
                .map(|dependency| {
                    (
                        dependency.edge().operand_index().unwrap(),
                        dependency.edge().sense(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{operator:?}");
            let assignment = graph.node(graph.assignment_id(0).unwrap()).unwrap();
            assert!(matches!(
                assignment.kind(),
                TimingNodeKind::Assignment(node)
                    if node.function() == AssignmentFunction::Operator(operator)
            ));
        }
    }

    #[test]
    fn direct_atoms_and_repeated_operands_preserve_real_parallel_dependencies() {
        let metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let direct = functional_cell("y", Expr::atom("a"), false);
        let provenance = vec![assignment_provenance(
            0,
            3,
            SourceAssignmentOrigin::Continuous,
            Vec::new(),
        )];
        let direct_graph = build_functional_timing_graph(&direct, &metadata, &provenance).unwrap();
        let direct_edges = direct_graph
            .dependencies()
            .iter()
            .filter(|edge| edge.edge().kind() == DependencyKind::Operand)
            .collect::<Vec<_>>();
        assert_eq!(direct_edges.len(), 1);
        assert_eq!(direct_edges[0].edge().operand_index(), Some(0));
        assert_eq!(direct_edges[0].edge().sense(), TimingSense::PositiveUnate);

        let repeated = functional_cell(
            "y",
            Expr::value(ValueOperator::And, vec![Expr::atom("a"), Expr::atom("a")]),
            false,
        );
        let repeated_graph =
            build_functional_timing_graph(&repeated, &metadata, &provenance).unwrap();
        assert_eq!(
            repeated_graph
                .dependencies()
                .iter()
                .filter(|edge| edge.edge().kind() == DependencyKind::Operand)
                .map(|edge| edge.edge().operand_index().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn functional_signal_order_uses_normalized_span_then_name_not_input_traversal() {
        let cell = functional_cell("y", Expr::atom("data"), false);
        let metadata = vec![
            signal_metadata("y", &[TimingSignalRole::Output], 3),
            signal_metadata("data", &[TimingSignalRole::Input], 2),
            signal_metadata("b", &[TimingSignalRole::Internal], 1),
            signal_metadata("a", &[TimingSignalRole::Internal], 1),
        ];
        let mut reversed = metadata.clone();
        reversed.reverse();
        let provenance = vec![assignment_provenance(
            0,
            4,
            SourceAssignmentOrigin::Continuous,
            Vec::new(),
        )];

        let first = build_functional_timing_graph(&cell, &metadata, &provenance).unwrap();
        let second = build_functional_timing_graph(&cell, &reversed, &provenance).unwrap();
        let signal_names = |graph: &TimingGraph| {
            graph
                .nodes()
                .filter_map(|node| match node.kind() {
                    TimingNodeKind::Signal(signal) => Some(signal.name().to_string()),
                    TimingNodeKind::Assignment(_) => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(signal_names(&first), vec!["a", "b", "data", "y"]);
        assert_eq!(signal_names(&first), signal_names(&second));
        assert_eq!(first.dependencies(), second.dependencies());
        assert_eq!(
            cut_register_cycles(&first).unwrap().topological_order(),
            cut_register_cycles(&second).unwrap().topological_order()
        );
    }

    #[test]
    fn transition_propagation_is_exact_only_when_the_contract_determines_it() {
        assert_eq!(
            propagate_transition(TimingSense::PositiveUnate, Transition::Rise),
            TransitionEffect::Exact(Transition::Rise)
        );
        assert_eq!(
            propagate_transition(TimingSense::PositiveUnate, Transition::Fall),
            TransitionEffect::Exact(Transition::Fall)
        );
        assert_eq!(
            propagate_transition(TimingSense::NegativeUnate, Transition::Rise),
            TransitionEffect::Exact(Transition::Fall)
        );
        assert_eq!(
            propagate_transition(TimingSense::NegativeUnate, Transition::Fall),
            TransitionEffect::Exact(Transition::Rise)
        );
        for sense in [TimingSense::NonUnate, TimingSense::Conditional] {
            assert_eq!(
                propagate_transition(sense, Transition::Rise),
                TransitionEffect::Indeterminate
            );
        }
        for sense in [
            TimingSense::PositiveUnate,
            TimingSense::NegativeUnate,
            TimingSense::NonUnate,
            TimingSense::Conditional,
            TimingSense::StateControl,
        ] {
            assert_eq!(
                propagate_transition(sense, Transition::TurnOff),
                TransitionEffect::Indeterminate
            );
        }
        assert_eq!(
            propagate_transition(TimingSense::StateControl, Transition::Rise),
            TransitionEffect::Exact(Transition::Rise)
        );
    }

    #[test]
    fn modeled_feedback_is_cut_only_at_its_state_boundary() {
        let cell = functional_cell("q", Expr::atom("q"), true);
        let metadata = vec![signal_metadata(
            "q",
            &[TimingSignalRole::Output, TimingSignalRole::ModeledRegister],
            1,
        )];
        let provenance = vec![assignment_provenance(
            0,
            2,
            SourceAssignmentOrigin::ProceduralStateful,
            Vec::new(),
        )];
        let graph = build_functional_timing_graph(&cell, &metadata, &provenance).unwrap();
        assert_eq!(graph.dependencies().len(), 2);
        let cut = cut_register_cycles(&graph).unwrap();
        assert_eq!(cut.excluded_state_boundaries().len(), 1);
        assert_eq!(cut.dependencies().len(), 1);
        assert_eq!(cut.topological_order().len(), 2);
    }

    #[test]
    fn ordinary_feedback_is_a_deterministic_source_spanned_cycle_error() {
        let cell = functional_cell("q", Expr::atom("q"), false);
        let metadata = vec![signal_metadata("q", &[TimingSignalRole::Output], 1)];
        let provenance = vec![assignment_provenance(
            0,
            8,
            SourceAssignmentOrigin::Continuous,
            Vec::new(),
        )];
        let first = build_functional_timing_graph(&cell, &metadata, &provenance).unwrap();
        let second = build_functional_timing_graph(&cell, &metadata, &provenance).unwrap();
        assert_eq!(
            first.nodes().cloned().collect::<Vec<_>>(),
            second.nodes().cloned().collect::<Vec<_>>()
        );
        assert_eq!(first.dependencies(), second.dependencies());

        let first_error = cut_register_cycles(&first).unwrap_err();
        let second_error = cut_register_cycles(&second).unwrap_err();
        assert_eq!(first_error, second_error);
        assert_eq!(first_error.span, span(8));
        assert_eq!(
            first_error.message,
            "combinational cycle remains after cutting modeled-state and multiply-driven resolved-net boundaries: n0, n1"
        );
    }

    #[test]
    fn resolved_boundaries_require_typed_multiple_drivers_and_never_override_state() {
        let two_drivers = resolved_cycle_cell(vec![Expr::atom("r"), Expr::atom("a")], false);
        let provenance = vec![
            assignment_provenance(0, 10, SourceAssignmentOrigin::Continuous, Vec::new()),
            assignment_provenance(1, 11, SourceAssignmentOrigin::Continuous, Vec::new()),
        ];
        let resolved_metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata(
                "r",
                &[TimingSignalRole::Output, TimingSignalRole::ResolvedNet],
                2,
            ),
        ];
        let resolved =
            build_functional_timing_graph(&two_drivers, &resolved_metadata, &provenance).unwrap();
        assert_eq!(
            resolved
                .dependencies()
                .iter()
                .filter(|dependency| {
                    dependency.edge().kind() == DependencyKind::ResolvedNetBoundary
                })
                .count(),
            2
        );
        let resolved_cut = cut_register_cycles(&resolved).unwrap();
        assert_eq!(resolved_cut.excluded_resolved_net_boundaries().len(), 2);
        assert!(resolved_cut.excluded_state_boundaries().is_empty());
        assert_eq!(
            resolved.dependencies().len(),
            resolved_cut.dependencies().len()
                + resolved_cut.excluded_resolved_net_boundaries().len(),
            "resolution boundaries remain first-class in the full graph"
        );

        let logic_metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata("r", &[TimingSignalRole::Output], 2),
        ];
        let ordinary =
            build_functional_timing_graph(&two_drivers, &logic_metadata, &provenance).unwrap();
        assert!(
            ordinary
                .dependencies()
                .iter()
                .all(|dependency| dependency.edge().kind() != DependencyKind::ResolvedNetBoundary)
        );
        assert!(cut_register_cycles(&ordinary).is_err());

        let single_driver = resolved_cycle_cell(vec![Expr::atom("r")], false);
        let single =
            build_functional_timing_graph(&single_driver, &resolved_metadata, &provenance[..1])
                .unwrap();
        assert!(
            single
                .dependencies()
                .iter()
                .all(|dependency| dependency.edge().kind() != DependencyKind::ResolvedNetBoundary)
        );
        assert!(
            cut_register_cycles(&single).is_err(),
            "a single-driver resolved self-loop remains an ordinary cycle"
        );

        let stateful_cell = resolved_cycle_cell(vec![Expr::atom("r"), Expr::atom("a")], true);
        let stateful_metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata(
                "r",
                &[
                    TimingSignalRole::Output,
                    TimingSignalRole::ResolvedNet,
                    TimingSignalRole::ModeledRegister,
                ],
                2,
            ),
        ];
        let stateful = build_functional_timing_graph(
            &stateful_cell,
            &stateful_metadata,
            &[
                assignment_provenance(
                    0,
                    10,
                    SourceAssignmentOrigin::ProceduralStateful,
                    Vec::new(),
                ),
                assignment_provenance(
                    1,
                    11,
                    SourceAssignmentOrigin::ProceduralStateful,
                    Vec::new(),
                ),
            ],
        )
        .unwrap();
        let stateful_cut = cut_register_cycles(&stateful).unwrap();
        assert_eq!(stateful_cut.excluded_state_boundaries().len(), 2);
        assert!(stateful_cut.excluded_resolved_net_boundaries().is_empty());
    }

    #[test]
    fn control_group_prefix_spans_divergent_targets_and_retains_duplicate_records() {
        let mut graph = TimingGraph::new();
        let source = graph
            .add_signal("source", roles(&[TimingSignalRole::Input]), span(1))
            .unwrap();
        let prefix = graph
            .add_signal("prefix", roles(&[TimingSignalRole::Internal]), span(2))
            .unwrap();
        let left = graph
            .add_signal("left", roles(&[TimingSignalRole::Output]), span(3))
            .unwrap();
        let right = graph
            .add_signal("right", roles(&[TimingSignalRole::Output]), span(4))
            .unwrap();

        let prefix_assignment = add_test_assignment(
            &mut graph,
            0,
            "prefix",
            AssignmentFunction::DirectAtom,
            &[(source, 0, TimingSense::PositiveUnate)],
            10,
        );
        let repeated_assignment = add_test_assignment(
            &mut graph,
            1,
            "left",
            AssignmentFunction::Operator(ValueOperator::And),
            &[
                (prefix, 0, TimingSense::PositiveUnate),
                (prefix, 1, TimingSense::PositiveUnate),
            ],
            11,
        );
        add_test_assignment(
            &mut graph,
            2,
            "right",
            AssignmentFunction::DirectAtom,
            &[(prefix, 0, TimingSense::PositiveUnate)],
            12,
        );
        for (order, target, line) in [(0, "left", 20), (1, "left", 21), (2, "right", 22)] {
            graph
                .add_constraint(
                    TimingConstraintSource::new(
                        order,
                        vec![control("source", line)],
                        target,
                        delay(&format!("T{order}")),
                        span(line),
                    )
                    .unwrap(),
                )
                .unwrap();
        }

        let repeated_edges = graph
            .dependencies()
            .iter()
            .filter(|dependency| {
                dependency.source() == prefix && dependency.target() == repeated_assignment
            })
            .count();
        assert_eq!(
            repeated_edges, 2,
            "parallel operands remain first-class edges"
        );

        let cut = cut_register_cycles(&graph).unwrap();
        let analysis = analyze_timing_graph(&graph, &cut).unwrap();
        let control_group = &analysis.control_groups()[0];
        assert_eq!(control_group.control_signal(), "source");
        assert_eq!(control_group.source_node(), source);
        assert_eq!(control_group.kind(), ControlGroupKind::MultipleTargets);
        assert_eq!(
            control_group
                .constraint_ids()
                .iter()
                .map(|id| id.ordinal())
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            control_group
                .control_ids()
                .iter()
                .map(|id| id.ordinal())
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(control_group.target_nodes(), &[left, right]);
        assert_eq!(control_group.common_prefix(), &[prefix, prefix_assignment]);

        let left_group = analysis
            .target_groups()
            .iter()
            .find(|group| group.group().target() == "left")
            .unwrap();
        assert_eq!(
            left_group
                .group()
                .constraint_ids()
                .iter()
                .map(|id| id.ordinal())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            left_group
                .reachable_controls()
                .iter()
                .map(|id| id.ordinal())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            left_group.common_suffix(),
            &[prefix, prefix_assignment, repeated_assignment],
            "duplicate source/target records do not change the node intersection"
        );
        assert!(
            left_group.reconvergent_nodes().is_empty(),
            "parallel edges from one predecessor are not distinct branches"
        );
    }

    #[test]
    fn target_group_suffix_spans_convergent_sources_and_differs_from_prefix() {
        let mut graph = TimingGraph::new();
        let source_a = graph
            .add_signal("source_a", roles(&[TimingSignalRole::Input]), span(1))
            .unwrap();
        let source_b = graph
            .add_signal("source_b", roles(&[TimingSignalRole::Input]), span(2))
            .unwrap();
        let left = graph
            .add_signal("left", roles(&[TimingSignalRole::Internal]), span(3))
            .unwrap();
        let right = graph
            .add_signal("right", roles(&[TimingSignalRole::Internal]), span(4))
            .unwrap();
        let merged = graph
            .add_signal("merged", roles(&[TimingSignalRole::Internal]), span(5))
            .unwrap();
        graph
            .add_signal("target", roles(&[TimingSignalRole::Output]), span(6))
            .unwrap();

        let left_assignment = add_test_assignment(
            &mut graph,
            0,
            "left",
            AssignmentFunction::DirectAtom,
            &[(source_a, 0, TimingSense::PositiveUnate)],
            10,
        );
        add_test_assignment(
            &mut graph,
            1,
            "right",
            AssignmentFunction::DirectAtom,
            &[(source_b, 0, TimingSense::PositiveUnate)],
            11,
        );
        let merge_assignment = add_test_assignment(
            &mut graph,
            2,
            "merged",
            AssignmentFunction::Operator(ValueOperator::And),
            &[
                (left, 0, TimingSense::PositiveUnate),
                (right, 1, TimingSense::PositiveUnate),
            ],
            12,
        );
        let target_assignment = add_test_assignment(
            &mut graph,
            3,
            "target",
            AssignmentFunction::DirectAtom,
            &[(merged, 0, TimingSense::PositiveUnate)],
            13,
        );
        for (order, source, line) in [(0, "source_a", 20), (1, "source_b", 21)] {
            graph
                .add_constraint(
                    TimingConstraintSource::new(
                        order,
                        vec![control(source, line)],
                        "target",
                        delay(&format!("T{order}")),
                        span(line),
                    )
                    .unwrap(),
                )
                .unwrap();
        }

        let cut = cut_register_cycles(&graph).unwrap();
        let analysis = analyze_timing_graph(&graph, &cut).unwrap();
        let target_group = &analysis.target_groups()[0];
        assert_eq!(
            target_group.common_suffix(),
            &[merged, merge_assignment, target_assignment]
        );
        assert_eq!(target_group.reconvergent_nodes(), &[merge_assignment]);

        let source_a_group = analysis
            .control_groups()
            .iter()
            .find(|group| group.control_signal() == "source_a")
            .unwrap();
        assert_eq!(source_a_group.kind(), ControlGroupKind::SingleTarget);
        assert_eq!(
            source_a_group.common_prefix(),
            &[
                left,
                merged,
                left_assignment,
                merge_assignment,
                target_assignment
            ]
        );
        assert_ne!(
            source_a_group.common_prefix(),
            target_group.common_suffix(),
            "cross-target prefixes and cross-source suffixes are distinct predicates"
        );
    }

    #[test]
    fn timing_report_is_identical_under_reversed_metadata_insertion_order() {
        let cell = Cell {
            name: "deterministic".to_string(),
            inputs: vec!["a".to_string(), "b".to_string()],
            outputs: vec!["y".to_string()],
            registers: Vec::new(),
            items: vec![CellItem::Assignment(Assignment {
                target: "y".to_string(),
                expr: Expr::value(ValueOperator::And, vec![Expr::atom("a"), Expr::atom("b")]),
                delay: delay("0"),
            })],
        };
        let mut metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata("b", &[TimingSignalRole::Input], 1),
            signal_metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![assignment_provenance(
            0,
            3,
            SourceAssignmentOrigin::Continuous,
            Vec::new(),
        )];
        let constraints = vec![
            TimingConstraintSource::new(0, vec![control("a", 4)], "y", delay("Ta"), span(4))
                .unwrap(),
            TimingConstraintSource::new(1, vec![control("b", 5)], "y", delay("Tb"), span(5))
                .unwrap(),
        ];
        let first = build_timing_graph(&cell, &metadata, &provenance, &constraints).unwrap();
        metadata.reverse();
        let second = build_timing_graph(&cell, &metadata, &provenance, &constraints).unwrap();
        let first_cut = cut_register_cycles(&first).unwrap();
        let second_cut = cut_register_cycles(&second).unwrap();
        let first_report = analyze_timing_graph(&first, &first_cut).unwrap();
        let second_report = analyze_timing_graph(&second, &second_cut).unwrap();

        assert_eq!(first_report, second_report);
        assert_eq!(first_report.render(), second_report.render());
        assert_eq!(
            first_report
                .control_groups()
                .iter()
                .map(TimingControlGroupReport::control_signal)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(!first_report.render().contains("NodeIndex"));
        assert!(first_report.render().contains("constraint p0 order=0"));
        assert!(
            first_report
                .render()
                .contains("control-group signal=a source=n0 kind=single-target")
        );
        assert!(
            first_report
                .render()
                .contains("group target=y kind=multiple")
        );
    }

    #[test]
    fn builder_rejects_nonflat_values_and_misaligned_provenance_at_source_spans() {
        let metadata = vec![
            signal_metadata("a", &[TimingSignalRole::Input], 1),
            signal_metadata("y", &[TimingSignalRole::Output], 2),
        ];
        let provenance = vec![assignment_provenance(
            0,
            9,
            SourceAssignmentOrigin::Continuous,
            Vec::new(),
        )];
        let nonflat = functional_cell(
            "y",
            Expr::List(vec![
                Expr::atom("not"),
                Expr::List(vec![Expr::atom("not"), Expr::atom("a")]),
            ]),
            false,
        );
        let error = build_functional_timing_graph(&nonflat, &metadata, &provenance).unwrap_err();
        assert_eq!(error.span, span(9));
        assert!(error.message.contains("invalid functional timing cell"));

        let direct = functional_cell("y", Expr::atom("a"), false);
        let error = build_functional_timing_graph(&direct, &metadata, &[]).unwrap_err();
        assert!(error.message.contains("provenance length mismatch"));
    }

    #[test]
    fn topology_generated_origin_and_role_are_distinct_and_typed() {
        let origin = AssignmentOrigin::GeneratedTopology {
            parent: SourceAssignmentOrigin::ProceduralStateful,
        };
        assert_eq!(origin.source(), SourceAssignmentOrigin::ProceduralStateful);
        assert!(origin.is_topology_generated());
        assert!(!origin.is_temporary());
        assert!(!origin.is_timing_identity());
        let provenance = AssignmentProvenance::new(0, 3, span(1), origin, Vec::new()).unwrap();
        assert_eq!(
            provenance.delay_origin(),
            AssignmentDelayOrigin::TopologyPlacement
        );
        assert_eq!(
            timing_signal_role_name(TimingSignalRole::TopologyTemporary),
            "topology-temporary"
        );
    }
}
