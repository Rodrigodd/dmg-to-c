//! Checked-in topology-overlay hints for timing paths whose physical topology
//! is absent from the baseline functional lowering.
//!
//! This module intentionally stops at the validation boundary.  A resolved
//! hint names only typed, validated overlay ingredients; it does not alter a
//! cell, timing decomposition, or ordinary lowering.  The later materializer
//! must consume [`ResolvedTopologyHint`] explicitly and turn every recipe into
//! an actual graph walk before a timing placement can be accepted.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::diagnostic::Span;
use crate::elaborate::GenerateMode;
use crate::ir::{CellItem, DelayTuple, Expr, LoweredModule, TimingExpr, ValueOperator};
use crate::timing_graph::{TimingConstraint, TimingGraph, Transition};

macro_rules! hint_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{} `{}`", $label, self.0)
            }
        }
    };
}

hint_id!(HintAssignmentId, "assignment");
hint_id!(HintSignalId, "signal");
hint_id!(HintPathRecipeId, "path recipe");
hint_id!(RoutingGuardId, "routing guard");
hint_id!(BaselineAssignmentId, "baseline assignment");

/// A source-located, actionable topology-hint error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyHintError {
    span: Span,
    message: String,
}

impl TopologyHintError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TopologyHintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: topology hint: {}",
            self.span.path.display(),
            self.span.line,
            self.span.column,
            self.message
        )
    }
}

impl std::error::Error for TopologyHintError {}

/// A value expression in the overlay's deliberately small, flat IR subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyValueExpr {
    Atom(String),
    Operation {
        operator: ValueOperator,
        operands: Vec<String>,
    },
}

impl TopologyValueExpr {
    pub fn to_expr(&self) -> Expr {
        match self {
            Self::Atom(atom) => Expr::atom(atom.clone()),
            Self::Operation { operator, operands } => Expr::value(
                *operator,
                operands.iter().cloned().map(Expr::atom).collect(),
            ),
        }
    }

    pub fn operands(&self) -> &[String] {
        match self {
            Self::Atom(atom) => std::slice::from_ref(atom),
            Self::Operation { operands, .. } => operands,
        }
    }
}

/// A timing component expressed as ordered existing alias/specparam terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayComponentTerms(Vec<String>);

impl DelayComponentTerms {
    pub fn terms(&self) -> &[String] {
        &self.0
    }
}

/// The source-order-preserving tuple carried by a generated assignment or
/// expected by a physical path recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyDelayTuple {
    One(DelayComponentTerms),
    Two {
        rise: DelayComponentTerms,
        fall: DelayComponentTerms,
    },
    Three {
        rise: DelayComponentTerms,
        fall: DelayComponentTerms,
        turn_off: DelayComponentTerms,
    },
}

impl TopologyDelayTuple {
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

    pub fn components(&self) -> Vec<&DelayComponentTerms> {
        match self {
            Self::One(one) => vec![one],
            Self::Two { rise, fall } => vec![rise, fall],
            Self::Three {
                rise,
                fall,
                turn_off,
            } => vec![rise, fall, turn_off],
        }
    }
}

/// A stable structural identifier for one retained specify control/target
/// transition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TopologyConstraintKey {
    path_order: usize,
    control_order: usize,
    control: String,
    target: String,
}

impl TopologyConstraintKey {
    pub fn control(&self) -> &str {
        &self.control
    }

    pub const fn path_order(&self) -> usize {
        self.path_order
    }

    pub const fn control_order(&self) -> usize {
        self.control_order
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

/// A structural baseline assignment anchor.  Both target and flat expression
/// are required, so no assignment ordinal or name heuristic is involved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineAssignmentAnchor {
    target: String,
    expression: TopologyValueExpr,
}

impl BaselineAssignmentAnchor {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn expression(&self) -> &TopologyValueExpr {
        &self.expression
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineAnchors {
    state: BaselineAssignmentAnchor,
    state_span: Span,
    outputs: Vec<BaselineAssignmentAnchor>,
    output_spans: Vec<Span>,
}

impl BaselineAnchors {
    pub fn state(&self) -> &BaselineAssignmentAnchor {
        &self.state
    }

    pub fn outputs(&self) -> &[BaselineAssignmentAnchor] {
        &self.outputs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologySignal {
    id: HintSignalId,
    name: String,
    span: Span,
}

impl TopologySignal {
    pub fn id(&self) -> &HintSignalId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyAssignment {
    id: HintAssignmentId,
    target: HintSignalId,
    expression: TopologyValueExpr,
    delay: TopologyDelayTuple,
    span: Span,
}

impl TopologyAssignment {
    pub fn id(&self) -> &HintAssignmentId {
        &self.id
    }

    pub fn target(&self) -> &HintSignalId {
        &self.target
    }

    pub fn expression(&self) -> &TopologyValueExpr {
        &self.expression
    }

    pub fn delay(&self) -> &TopologyDelayTuple {
        &self.delay
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

/// An explicit exception for a routing-only dependency which cannot be part of
/// a functional timing walk.  Recipes must name this guard to omit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingGuardAnnotation {
    id: RoutingGuardId,
    edge: TopologyDependencyEdge,
    reason: RoutingGuardReason,
    span: Span,
}

impl RoutingGuardAnnotation {
    pub fn id(&self) -> &RoutingGuardId {
        &self.id
    }

    pub fn edge(&self) -> &TopologyDependencyEdge {
        &self.edge
    }

    pub const fn reason(&self) -> RoutingGuardReason {
        self.reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyDependencyEdge {
    assignment: TopologyAssignmentRef,
    operand_index: usize,
}

impl TopologyDependencyEdge {
    pub fn assignment(&self) -> &TopologyAssignmentRef {
        &self.assignment
    }
    pub const fn operand_index(&self) -> usize {
        self.operand_index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingGuardReason {
    Routing,
    Knownness,
    ExactFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyPathRecipe {
    id: HintPathRecipeId,
    key: TopologyConstraintKey,
    target_transition: Transition,
    steps: Vec<TopologyPathStep>,
    expected_terms: DelayComponentTerms,
    omitted_routing_guards: Vec<RoutingGuardId>,
    span: Span,
}

impl TopologyPathRecipe {
    pub fn id(&self) -> &HintPathRecipeId {
        &self.id
    }

    pub fn key(&self) -> &TopologyConstraintKey {
        &self.key
    }

    pub const fn target_transition(&self) -> Transition {
        self.target_transition
    }

    pub fn steps(&self) -> &[TopologyPathStep] {
        &self.steps
    }

    pub fn expected_terms(&self) -> &DelayComponentTerms {
        &self.expected_terms
    }
}

/// A dependency hop that must be present in the future materialized graph.
/// `operand_index` is a durable consumed-edge identity, not a signal spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyPathStep {
    assignment: TopologyAssignmentRef,
    operand_index: usize,
    transition: Transition,
}

impl TopologyPathStep {
    pub fn assignment(&self) -> &TopologyAssignmentRef {
        &self.assignment
    }

    pub const fn operand_index(&self) -> usize {
        self.operand_index
    }

    pub const fn transition(&self) -> Transition {
        self.transition
    }
}

/// A step either consumes an existing structural anchor or one declared
/// generated assignment.  Raw names are never retained at this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyAssignmentRef {
    BaselineId(BaselineAssignmentId),
    Rewrite(BaselineAssignmentId),
    Generated(HintAssignmentId),
}

/// Parsed and statically well-formed overlay intent.  It deliberately has no
/// links to baseline IR names until [`TopologyHintCatalog::resolve`] is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyHint {
    module: String,
    generate_mode: GenerateMode,
    baseline: BaselineAnchors,
    baseline_assignments: Vec<TopologyBaselineAssignment>,
    signals: Vec<TopologySignal>,
    assignments: Vec<TopologyAssignment>,
    routing_guards: Vec<RoutingGuardAnnotation>,
    path_recipes: Vec<TopologyPathRecipe>,
    rewrites: Vec<TopologyRewrite>,
    span: Span,
}

/// Stable named original-assignment snapshot.  Steps, guards, and rewrites
/// resolve through this collection rather than an assignment ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyBaselineAssignment {
    id: BaselineAssignmentId,
    anchor: BaselineAssignmentAnchor,
    span: Span,
}

impl TopologyBaselineAssignment {
    pub fn id(&self) -> &BaselineAssignmentId {
        &self.id
    }
    pub fn anchor(&self) -> &BaselineAssignmentAnchor {
        &self.anchor
    }
}

impl TopologyHint {
    pub fn module(&self) -> &str {
        &self.module
    }

    pub const fn generate_mode(&self) -> GenerateMode {
        self.generate_mode
    }

    pub fn baseline(&self) -> &BaselineAnchors {
        &self.baseline
    }

    pub fn baseline_assignments(&self) -> &[TopologyBaselineAssignment] {
        &self.baseline_assignments
    }

    pub fn signals(&self) -> &[TopologySignal] {
        &self.signals
    }

    pub fn assignments(&self) -> &[TopologyAssignment] {
        &self.assignments
    }

    pub fn path_recipes(&self) -> &[TopologyPathRecipe] {
        &self.path_recipes
    }

    pub fn rewrites(&self) -> &[TopologyRewrite] {
        &self.rewrites
    }
}

/// Declarative exact replacement contract for a baseline state/output driver.
/// The fallback is a complete structural snapshot, not a free-form overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyRewrite {
    baseline: BaselineAssignmentId,
    anchor: BaselineAssignmentAnchor,
    replacement: HintAssignmentId,
    fallback: HintAssignmentId,
    knownness_guard: RoutingGuardId,
    exact_fallback_guard: RoutingGuardId,
    span: Span,
}

impl TopologyRewrite {
    pub fn anchor(&self) -> &BaselineAssignmentAnchor {
        &self.anchor
    }
    pub fn replacement(&self) -> &HintAssignmentId {
        &self.replacement
    }
    pub fn fallback(&self) -> &HintAssignmentId {
        &self.fallback
    }

    pub fn baseline(&self) -> &BaselineAssignmentId {
        &self.baseline
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyHintCatalog {
    hints: Vec<TopologyHint>,
}

/// The checked-in delayful dmg_dffsr overlay. Keep this repository-relative
/// path stable: diagnostics from the built-in catalog must be actionable in a
/// source checkout without consulting the host filesystem at runtime.
pub const DMG_DFFSR_HINT_PATH: &str = "sv-to-sexpr/topology-hints/dmg_dffsr.toml";

/// Returns the embedded source text for corruption and diagnostic tests.
pub const fn builtin_dmg_dffsr_hint_source() -> &'static str {
    include_str!("../topology-hints/dmg_dffsr.toml")
}

/// Parses the deterministic, checked-in topology catalog without runtime file
/// access. New built-ins should be appended here in stable source order.
pub fn builtin_topology_hint_catalog() -> Result<TopologyHintCatalog, TopologyHintError> {
    TopologyHintCatalog::parse(DMG_DFFSR_HINT_PATH, builtin_dmg_dffsr_hint_source())
}

impl TopologyHintCatalog {
    /// Parses only checked-in TOML schema.  Reference and baseline validation
    /// is intentionally deferred to [`Self::resolve`].
    pub fn parse(path: impl Into<PathBuf>, input: &str) -> Result<Self, TopologyHintError> {
        let path = path.into();
        let raw: RawCatalog = toml::from_str(input).map_err(|error| {
            let offset = error.span().map_or(0, |span| span.start);
            TopologyHintError::new(span_at_offset(&path, input, offset), error.to_string())
        })?;
        let mut hints = Vec::with_capacity(raw.hints.len());
        for raw_hint in raw.hints {
            let span = span_at_offset(&path, input, raw_hint.span().start);
            hints.push(TopologyHint::from_raw(
                raw_hint.into_inner(),
                span,
                &path,
                input,
            )?);
        }
        let mut seen = BTreeSet::new();
        for hint in &hints {
            if !seen.insert((hint.module.clone(), hint.generate_mode.label())) {
                return Err(TopologyHintError::new(
                    hint.span.clone(),
                    format!(
                        "duplicate hint for module `{}` in {} mode",
                        hint.module, hint.generate_mode
                    ),
                ));
            }
        }
        Ok(Self { hints })
    }

    pub fn hints(&self) -> &[TopologyHint] {
        &self.hints
    }

    /// Resolves a matching overlay when one is checked in. Absence is an
    /// ordinary deterministic result for generic cells.
    pub fn resolve_optional(
        &self,
        context: &TopologyHintContext<'_>,
    ) -> Result<Option<ResolvedTopologyHintCatalog>, TopologyHintError> {
        if context.lowered.cell.name != context.module {
            return Err(TopologyHintError::new(
                Span::new("<topology-hint-context>", 1, 1),
                format!(
                    "context module {} contradicts lowered cell {}",
                    context.module, context.lowered.cell.name
                ),
            ));
        }
        let matches = self
            .hints
            .iter()
            .filter(|hint| {
                hint.module == context.lowered.cell.name
                    && hint.generate_mode == context.generate_mode
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [hint] => Ok(Some(ResolvedTopologyHintCatalog {
                hints: vec![resolve_hint(hint, context)?],
            })),
            _ => Err(TopologyHintError::new(
                Span::new("<topology-hint-selection>", 1, 1),
                format!(
                    "ambiguous topology hints for module {} in {} mode",
                    context.lowered.cell.name, context.generate_mode
                ),
            )),
        }
    }

    /// Resolves every baseline-dependent reference.  A later topology
    /// materializer receives only [`ResolvedTopologyHintCatalog`], making it
    /// impossible to bypass these checks accidentally.
    pub fn resolve(
        &self,
        context: &TopologyHintContext<'_>,
    ) -> Result<ResolvedTopologyHintCatalog, TopologyHintError> {
        self.resolve_optional(context)?.ok_or_else(|| {
            TopologyHintError::new(
                Span::new("<topology-hint-selection>", 1, 1),
                format!(
                    "no topology hint for module `{}` in {} mode",
                    context.lowered.cell.name, context.generate_mode
                ),
            )
        })
    }
}

/// The explicit baseline boundary used for hint resolution.
pub struct TopologyHintContext<'a> {
    module: &'a str,
    generate_mode: GenerateMode,
    lowered: &'a LoweredModule,
    graph: &'a TimingGraph,
}

impl<'a> TopologyHintContext<'a> {
    pub fn new(
        module: &'a str,
        generate_mode: GenerateMode,
        lowered: &'a LoweredModule,
        graph: &'a TimingGraph,
    ) -> Self {
        Self {
            module,
            generate_mode,
            lowered,
            graph,
        }
    }
}

/// Fully typed overlay data whose aliases, baseline anchors, constraint keys,
/// and routing omissions have all been checked against one exact baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopologyHint {
    hint: TopologyHint,
    signals: Vec<ResolvedTopologySignal>,
    assignments: Vec<ResolvedTopologyAssignment>,
    baseline_assignments: Vec<ResolvedBaselineAssignment>,
    guards: Vec<ResolvedRoutingGuard>,
    rewrites: Vec<ResolvedTopologyRewrite>,
    recipes: Vec<ResolvedPathRecipe>,
    alias_terms: BTreeMap<String, TimingExpr>,
    constraint_paths: BTreeMap<TopologyConstraintKey, usize>,
}

impl ResolvedTopologyHint {
    pub fn module(&self) -> &str {
        self.hint.module()
    }

    pub fn hint(&self) -> &TopologyHint {
        &self.hint
    }

    pub fn assignments(&self) -> &[ResolvedTopologyAssignment] {
        &self.assignments
    }

    pub fn signals(&self) -> &[ResolvedTopologySignal] {
        &self.signals
    }

    pub fn signal(&self, id: &HintSignalId) -> Option<&ResolvedTopologySignal> {
        self.signals.iter().find(|signal| &signal.id == id)
    }

    pub fn baseline_assignments(&self) -> &[ResolvedBaselineAssignment] {
        &self.baseline_assignments
    }
    pub fn guards(&self) -> &[ResolvedRoutingGuard] {
        &self.guards
    }
    pub fn rewrites(&self) -> &[ResolvedTopologyRewrite] {
        &self.rewrites
    }
    pub fn recipes(&self) -> &[ResolvedPathRecipe] {
        &self.recipes
    }
    pub fn assignment(&self, id: &HintAssignmentId) -> Option<&ResolvedTopologyAssignment> {
        self.assignments.iter().find(|value| &value.id == id)
    }
    pub fn baseline_assignment(
        &self,
        id: &BaselineAssignmentId,
    ) -> Option<&ResolvedBaselineAssignment> {
        self.baseline_assignments
            .iter()
            .find(|value| &value.id == id)
    }
    pub fn guard(&self, id: &RoutingGuardId) -> Option<&ResolvedRoutingGuard> {
        self.guards.iter().find(|value| &value.id == id)
    }
    pub fn rewrite(&self, id: &BaselineAssignmentId) -> Option<&ResolvedTopologyRewrite> {
        self.rewrites.iter().find(|value| &value.baseline == id)
    }
    pub fn recipe(&self, id: &HintPathRecipeId) -> Option<&ResolvedPathRecipe> {
        self.recipes.iter().find(|value| &value.id == id)
    }

    pub fn alias_terms(&self) -> &BTreeMap<String, TimingExpr> {
        &self.alias_terms
    }

    /// The source timing path order for each recipe key, in deterministic
    /// recipe/source order.
    pub fn constraint_paths(&self) -> &BTreeMap<TopologyConstraintKey, usize> {
        &self.constraint_paths
    }

    /// Phase 5 must call this boundary before creating overlay assignments.
    /// There is no API which turns a parsed hint directly into a placement.
    pub fn require_materialization(&self) -> TopologyMaterializationBoundary<'_> {
        TopologyMaterializationBoundary { hint: self }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopologyAssignment {
    id: HintAssignmentId,
    target: HintSignalId,
    target_name: String,
    expression: TopologyValueExpr,
    operands: Vec<TopologyOperandRef>,
    delay: DelayTuple,
    span: Span,
}

impl ResolvedTopologyAssignment {
    pub fn id(&self) -> &HintAssignmentId {
        &self.id
    }
    pub fn operands(&self) -> &[TopologyOperandRef] {
        &self.operands
    }
    pub fn target(&self) -> &HintSignalId {
        &self.target
    }
    pub fn target_name(&self) -> &str {
        &self.target_name
    }
    pub fn expression(&self) -> &TopologyValueExpr {
        &self.expression
    }
    pub fn delay(&self) -> &DelayTuple {
        &self.delay
    }
    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopologySignal {
    id: HintSignalId,
    name: String,
    span: Span,
}

impl ResolvedTopologySignal {
    pub fn id(&self) -> &HintSignalId {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBaselineAssignment {
    id: BaselineAssignmentId,
    anchor: BaselineAssignmentAnchor,
    item_order: usize,
    assignment_order: usize,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoutingGuard {
    pub id: RoutingGuardId,
    pub assignment: HintAssignmentId,
    pub operand_index: usize,
    pub reason: RoutingGuardReason,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopologyRewrite {
    pub baseline: BaselineAssignmentId,
    pub replacement: HintAssignmentId,
    pub fallback: HintAssignmentId,
    pub knownness_guard: RoutingGuardId,
    pub exact_fallback_guard: RoutingGuardId,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPathStepKind {
    Baseline(BaselineAssignmentId),
    Generated(HintAssignmentId),
    Rewrite(BaselineAssignmentId),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPathStep {
    pub kind: ResolvedPathStepKind,
    pub operand_index: usize,
    pub transition: Transition,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPathRecipe {
    pub id: HintPathRecipeId,
    pub span: Span,
    pub path_order: usize,
    pub control_order: usize,
    pub target: String,
    pub transition: Transition,
    pub ingress: ResolvedRecipeIngress,
    pub expected_terms: DelayComponentTerms,
    pub expected: TimingExpr,
    pub steps: Vec<ResolvedPathStep>,
    pub omitted_guards: Vec<RoutingGuardId>,
}

/// The typed boundary through which a retained specify control enters one
/// physical overlay walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedRecipeIngress {
    DirectControl,
    BaselineBuffer(BaselineAssignmentId),
}

impl ResolvedBaselineAssignment {
    pub fn id(&self) -> &BaselineAssignmentId {
        &self.id
    }
    pub fn anchor(&self) -> &BaselineAssignmentAnchor {
        &self.anchor
    }
    pub const fn item_order(&self) -> usize {
        self.item_order
    }
    pub const fn assignment_order(&self) -> usize {
        self.assignment_order
    }
    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyOperandRef {
    BaselineSignal(String),
    GeneratedSignal(HintSignalId),
    LogicAtom(crate::ir::LogicValue),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopologyHintCatalog {
    hints: Vec<ResolvedTopologyHint>,
}

impl ResolvedTopologyHintCatalog {
    pub fn hints(&self) -> &[ResolvedTopologyHint] {
        &self.hints
    }
}

/// Explicit unresolved-to-materialized boundary.  It carries no mutable cell
/// or decomposition capability: future code must build actual assignments and
/// then independently resolve each recipe's graph walk.
pub struct TopologyMaterializationBoundary<'a> {
    hint: &'a ResolvedTopologyHint,
}

impl<'a> TopologyMaterializationBoundary<'a> {
    pub fn hint(&self) -> &'a ResolvedTopologyHint {
        self.hint
    }
}

impl TopologyHint {
    fn from_raw(
        raw: RawHint,
        span: Span,
        path: &Path,
        input: &str,
    ) -> Result<Self, TopologyHintError> {
        let generate_mode = parse_mode(&raw.generate_mode, &span)?;
        let baseline_raw = raw.baseline.into_inner();
        let state_span = span_at_offset(path, input, baseline_raw.state.span().start);
        let state = baseline_raw.state.into_inner().into_anchor(&state_span)?;
        let mut output_spans = Vec::new();
        let outputs = baseline_raw
            .outputs
            .into_iter()
            .map(|anchor| {
                let output_span = span_at_offset(path, input, anchor.span().start);
                output_spans.push(output_span.clone());
                anchor.into_inner().into_anchor(&output_span)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let baseline = BaselineAnchors {
            state,
            state_span,
            outputs,
            output_spans,
        };
        if baseline.outputs.is_empty() {
            return Err(TopologyHintError::new(
                span,
                "baseline.outputs must contain at least one anchored output assignment",
            ));
        }
        let baseline_assignments = raw
            .baseline_assignments
            .into_iter()
            .map(|assignment| {
                let assignment_span = span_at_offset(path, input, assignment.span().start);
                let assignment = assignment.into_inner();
                Ok(TopologyBaselineAssignment {
                    id: BaselineAssignmentId(id(
                        assignment.id,
                        "baseline assignment",
                        &assignment_span,
                    )?),
                    anchor: RawAnchor {
                        target: assignment.target,
                        expression: assignment.expression,
                    }
                    .into_anchor(&assignment_span)?,
                    span: assignment_span,
                })
            })
            .collect::<Result<Vec<_>, TopologyHintError>>()?;
        let mut baseline_ids = BTreeSet::new();
        for assignment in &baseline_assignments {
            if !baseline_ids.insert(assignment.id.clone()) {
                return Err(TopologyHintError::new(
                    assignment.span.clone(),
                    format!(
                        "duplicate baseline assignment ID `{}`",
                        assignment.id.as_str()
                    ),
                ));
            }
        }
        let signals = raw
            .signals
            .into_iter()
            .map(|signal| {
                let signal_span = span_at_offset(path, input, signal.span().start);
                let signal = signal.into_inner();
                Ok(TopologySignal {
                    id: id(signal.id, "signal", &signal_span).map(HintSignalId)?,
                    name: atom(signal.name, "signal name", &signal_span)?,
                    span: signal_span,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut signal_ids = BTreeSet::new();
        let mut signal_names = BTreeSet::new();
        for signal in &signals {
            if !signal_ids.insert(signal.id.as_str()) {
                return Err(TopologyHintError::new(
                    signal.span.clone(),
                    format!("duplicate signal ID `{}`", signal.id.as_str()),
                ));
            }
            if !signal_names.insert(signal.name.as_str()) {
                return Err(TopologyHintError::new(
                    signal.span.clone(),
                    format!("duplicate signal name `{}`", signal.name),
                ));
            }
        }

        let assignments = raw
            .assignments
            .into_iter()
            .map(|assignment| {
                let assignment_span = span_at_offset(path, input, assignment.span().start);
                let assignment = assignment.into_inner();
                Ok(TopologyAssignment {
                    id: HintAssignmentId(id(assignment.id, "assignment", &assignment_span)?),
                    target: HintSignalId(id(
                        assignment.target,
                        "assignment target",
                        &assignment_span,
                    )?),
                    expression: assignment.expression.into_expression(&assignment_span)?,
                    delay: assignment.delay.into_delay(&assignment_span)?,
                    span: assignment_span,
                })
            })
            .collect::<Result<Vec<_>, TopologyHintError>>()?;
        let mut assignment_ids = BTreeSet::new();
        for assignment in &assignments {
            if !assignment_ids.insert(assignment.id.as_str()) {
                return Err(TopologyHintError::new(
                    assignment.span.clone(),
                    format!("duplicate assignment ID `{}`", assignment.id.as_str()),
                ));
            }
        }

        let routing_guards = raw
            .routing_guards
            .into_iter()
            .map(|guard| {
                let guard_span = span_at_offset(path, input, guard.span().start);
                let guard = guard.into_inner();
                Ok(RoutingGuardAnnotation {
                    id: RoutingGuardId(id(guard.id, "routing guard", &guard_span)?),
                    edge: TopologyDependencyEdge {
                        assignment: guard.assignment.into_assignment_ref(&guard_span)?,
                        operand_index: guard.operand_index,
                    },
                    reason: parse_guard_reason(&guard.reason, &guard_span)?,
                    span: guard_span,
                })
            })
            .collect::<Result<Vec<_>, TopologyHintError>>()?;
        let mut guard_ids = BTreeSet::new();
        for guard in &routing_guards {
            if !guard_ids.insert(guard.id.as_str()) {
                return Err(TopologyHintError::new(
                    guard.span.clone(),
                    format!("duplicate routing guard ID `{}`", guard.id.as_str()),
                ));
            }
        }

        let path_recipes = raw
            .path_recipes
            .into_iter()
            .map(|recipe| {
                let recipe_span = span_at_offset(path, input, recipe.span().start);
                let recipe = recipe.into_inner();
                Ok(TopologyPathRecipe {
                    id: HintPathRecipeId(id(recipe.id, "path recipe", &recipe_span)?),
                    key: recipe.key.into_key(&recipe_span)?,
                    target_transition: parse_transition(recipe.target_transition, &recipe_span)?,
                    steps: recipe
                        .steps
                        .into_iter()
                        .map(|step| step.into_step(&recipe_span))
                        .collect::<Result<_, _>>()?,
                    expected_terms: DelayComponentTerms(
                        recipe
                            .expected_terms
                            .into_iter()
                            .map(|value| atom(value, "path recipe timing term", &recipe_span))
                            .collect::<Result<_, _>>()?,
                    ),
                    omitted_routing_guards: recipe
                        .omitted_routing_guards
                        .into_iter()
                        .map(|value| {
                            id(value, "omitted routing guard", &recipe_span).map(RoutingGuardId)
                        })
                        .collect::<Result<_, _>>()?,
                    span: recipe_span,
                })
            })
            .collect::<Result<Vec<_>, TopologyHintError>>()?;
        let mut recipe_ids = BTreeSet::new();
        for recipe in &path_recipes {
            if !recipe_ids.insert(recipe.id.as_str()) {
                return Err(TopologyHintError::new(
                    recipe.span.clone(),
                    format!("duplicate path recipe ID `{}`", recipe.id.as_str()),
                ));
            }
        }
        let rewrites = raw
            .rewrites
            .into_iter()
            .map(|rewrite| {
                let rewrite_span = span_at_offset(path, input, rewrite.span().start);
                let rewrite = rewrite.into_inner();
                let anchor_id =
                    BaselineAssignmentId(id(rewrite.anchor_id, "rewrite anchor", &rewrite_span)?);
                let anchor = baseline_assignments
                    .iter()
                    .find(|assignment| assignment.id == anchor_id)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            rewrite_span.clone(),
                            format!("rewrite references missing {anchor_id}"),
                        )
                    })?
                    .anchor
                    .clone();
                Ok(TopologyRewrite {
                    baseline: anchor_id,
                    anchor,
                    replacement: HintAssignmentId(id(
                        rewrite.replacement,
                        "rewrite replacement",
                        &rewrite_span,
                    )?),
                    fallback: HintAssignmentId(id(
                        rewrite.fallback,
                        "rewrite fallback",
                        &rewrite_span,
                    )?),
                    knownness_guard: RoutingGuardId(id(
                        rewrite.knownness_guard,
                        "rewrite knownness guard",
                        &rewrite_span,
                    )?),
                    exact_fallback_guard: RoutingGuardId(id(
                        rewrite.exact_fallback_guard,
                        "rewrite exact fallback guard",
                        &rewrite_span,
                    )?),
                    span: rewrite_span,
                })
            })
            .collect::<Result<Vec<_>, TopologyHintError>>()?;

        Ok(Self {
            module: atom(raw.module, "module", &span)?,
            generate_mode,
            baseline,
            baseline_assignments,
            signals,
            assignments,
            routing_guards,
            path_recipes,
            rewrites,
            span,
        })
    }
}

fn resolve_hint(
    hint: &TopologyHint,
    context: &TopologyHintContext<'_>,
) -> Result<ResolvedTopologyHint, TopologyHintError> {
    if hint.module != context.module || hint.generate_mode != context.generate_mode {
        return Err(TopologyHintError::new(
            hint.span.clone(),
            format!(
                "hint is for module `{}` in {} mode, not `{}` in {} mode",
                hint.module, hint.generate_mode, context.module, context.generate_mode
            ),
        ));
    }

    let baseline_names = collect_baseline_names(context.lowered);
    let resolved_baselines = resolve_baseline_assignments(hint, context.lowered)?;
    for (label, anchor, role_span) in
        std::iter::once(("state", &hint.baseline.state, &hint.baseline.state_span)).chain(
            hint.baseline
                .outputs
                .iter()
                .zip(&hint.baseline.output_spans)
                .map(|(anchor, span)| ("output", anchor, span)),
        )
    {
        let count = resolved_baselines
            .iter()
            .filter(|baseline| baseline.anchor == *anchor)
            .count();
        if count != 1 {
            return Err(TopologyHintError::new(
                role_span.clone(),
                format!("baseline {label} role must map to exactly one named baseline assignment"),
            ));
        }
    }
    validate_anchor(
        &hint.baseline.state,
        context.lowered,
        true,
        &hint.baseline.state_span,
        "state",
    )?;
    for (anchor, role_span) in hint
        .baseline
        .outputs
        .iter()
        .zip(&hint.baseline.output_spans)
    {
        if !context
            .lowered
            .cell
            .outputs
            .iter()
            .any(|output| output == &anchor.target)
        {
            return Err(TopologyHintError::new(
                role_span.clone(),
                format!(
                    "baseline output anchor `{}` is not an output",
                    anchor.target
                ),
            ));
        }
        validate_anchor(anchor, context.lowered, false, role_span, "output")?;
    }

    let mut generated_names = BTreeSet::new();
    let signal_ids = hint
        .signals
        .iter()
        .map(|signal| (signal.id.clone(), signal.name.clone()))
        .collect::<BTreeMap<_, _>>();
    for signal in &hint.signals {
        if baseline_names.contains(&signal.name) || is_reserved_timing_name(&signal.name) {
            return Err(TopologyHintError::new(
                signal.span.clone(),
                format!(
                    "generated signal `{}` collides with the baseline or reserved timing names",
                    signal.name
                ),
            ));
        }
        if !generated_names.insert(signal.name.clone()) {
            return Err(TopologyHintError::new(
                signal.span.clone(),
                format!("duplicate generated signal name `{}`", signal.name),
            ));
        }
    }
    let assignment_ids = hint
        .assignments
        .iter()
        .map(|assignment| assignment.id.clone())
        .collect::<BTreeSet<_>>();
    let assignment_by_id = hint
        .assignments
        .iter()
        .map(|assignment| (&assignment.id, assignment))
        .collect::<BTreeMap<_, _>>();
    let guard_by_id = hint
        .routing_guards
        .iter()
        .map(|guard| (&guard.id, guard))
        .collect::<BTreeMap<_, _>>();
    let mut rewritten = BTreeSet::new();
    for rewrite in &hint.rewrites {
        let declared = rewrite.anchor == hint.baseline.state
            || hint
                .baseline
                .outputs
                .iter()
                .any(|output| output == &rewrite.anchor);
        if !declared {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                "rewrite anchor is not a state/output baseline role",
            ));
        }
        let Some(replacement) = assignment_by_id.get(&rewrite.replacement) else {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                format!("rewrite references missing {}", rewrite.replacement),
            ));
        };
        let Some(fallback) = assignment_by_id.get(&rewrite.fallback) else {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                format!("rewrite references missing fallback {}", rewrite.fallback),
            ));
        };
        let fallback_target = hint
            .signals
            .iter()
            .find(|signal| signal.id == fallback.target)
            .ok_or_else(|| {
                TopologyHintError::new(
                    rewrite.span.clone(),
                    format!(
                        "fallback {} targets missing signal {}",
                        rewrite.fallback, fallback.target
                    ),
                )
            })?
            .name
            .as_str();
        if fallback.expression != rewrite.anchor.expression
            || !fallback
                .delay
                .components()
                .iter()
                .all(|component| component.terms().is_empty())
        {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                "rewrite fallback assignment must be a zero-delay exact baseline-expression snapshot",
            ));
        }
        let TopologyValueExpr::Operation {
            operator: ValueOperator::Mux,
            operands,
        } = &replacement.expression
        else {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                "rewrite replacement must be a flat mux(knownness, physical, fallback)",
            ));
        };
        if operands.get(2).map(String::as_str) != Some(fallback_target) {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                "rewrite mux operand 2 must be the named fallback assignment target",
            ));
        }
        for (guard_id, expected_index, expected_reason) in [
            (&rewrite.knownness_guard, 0, RoutingGuardReason::Knownness),
            (
                &rewrite.exact_fallback_guard,
                2,
                RoutingGuardReason::ExactFallback,
            ),
        ] {
            let Some(guard) = guard_by_id.get(guard_id) else {
                return Err(TopologyHintError::new(
                    rewrite.span.clone(),
                    format!("rewrite references missing {guard_id}"),
                ));
            };
            if guard.reason != expected_reason
                || guard.edge.assignment
                    != TopologyAssignmentRef::Generated(rewrite.replacement.clone())
                || guard.edge.operand_index != expected_index
            {
                return Err(TopologyHintError::new(
                    rewrite.span.clone(),
                    "rewrite guard does not match its required mux operand",
                ));
            }
        }
        if !rewritten.insert(rewrite.baseline.clone()) {
            return Err(TopologyHintError::new(
                rewrite.span.clone(),
                format!("duplicate rewrite for baseline `{}`", rewrite.anchor.target),
            ));
        }
    }
    for (anchor, role_span) in std::iter::once((&hint.baseline.state, &hint.baseline.state_span))
        .chain(
            hint.baseline
                .outputs
                .iter()
                .zip(&hint.baseline.output_spans),
        )
    {
        let baseline = hint
            .baseline_assignments
            .iter()
            .find(|assignment| assignment.anchor == *anchor)
            .map(|assignment| &assignment.id);
        if baseline.is_none_or(|baseline| !rewritten.contains(baseline)) {
            return Err(TopologyHintError::new(
                role_span.clone(),
                format!("missing rewrite for baseline `{}`", anchor.target),
            ));
        }
    }
    for assignment in &hint.assignments {
        if !signal_ids.contains_key(&assignment.target) {
            return Err(TopologyHintError::new(
                assignment.span.clone(),
                format!(
                    "{} targets missing generated signal {}",
                    assignment.id, assignment.target
                ),
            ));
        }
        validate_overlay_expression(
            &assignment.expression,
            &baseline_names,
            &signal_ids,
            &assignment.span,
        )?;
    }
    let assignments = hint
        .assignments
        .iter()
        .map(|assignment| {
            let operands = assignment
                .expression
                .operands()
                .iter()
                .map(|operand| {
                    if let Some((id, _)) = signal_ids.iter().find(|(_, name)| *name == operand) {
                        return Ok(TopologyOperandRef::GeneratedSignal(id.clone()));
                    }
                    if let Some(value) = logic_atom(operand) {
                        return Ok(TopologyOperandRef::LogicAtom(value));
                    }
                    if baseline_names.contains(operand) {
                        return Ok(TopologyOperandRef::BaselineSignal(operand.clone()));
                    }
                    Err(TopologyHintError::new(
                        assignment.span.clone(),
                        format!("overlay expression references unknown atom `{operand}`"),
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTopologyAssignment {
                id: assignment.id.clone(),
                target: assignment.target.clone(),
                target_name: signal_ids.get(&assignment.target).cloned().ok_or_else(|| {
                    TopologyHintError::new(
                        assignment.span.clone(),
                        format!(
                            "{} targets missing signal {}",
                            assignment.id, assignment.target
                        ),
                    )
                })?,
                expression: assignment.expression.clone(),
                operands,
                delay: resolve_topology_delay(
                    &assignment.delay,
                    context.lowered,
                    &assignment.span,
                )?,
                span: assignment.span.clone(),
            })
        })
        .collect::<Result<Vec<_>, TopologyHintError>>()?;

    let mut alias_terms = BTreeMap::new();
    for assignment in &hint.assignments {
        resolve_delay_terms(
            &assignment.delay,
            context.lowered,
            &mut alias_terms,
            &assignment.span,
        )?;
    }
    for recipe in &hint.path_recipes {
        for term in recipe.expected_terms.terms() {
            let Some(value) = context.lowered.timing_aliases.get(term) else {
                return Err(TopologyHintError::new(
                    recipe.span.clone(),
                    format!("timing term `{term}` is not a resolved alias or specparam"),
                ));
            };
            alias_terms.insert(term.clone(), value.clone());
        }
    }

    let guards = hint
        .routing_guards
        .iter()
        .map(|guard| (guard.id.clone(), guard))
        .collect::<BTreeMap<_, _>>();
    let rewrite_cones = hint
        .rewrites
        .iter()
        .map(|rewrite| {
            (
                rewrite.baseline.clone(),
                generated_cone(&rewrite.replacement, hint, &signal_ids),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for guard in &hint.routing_guards {
        validate_dependency_edge(
            &guard.edge,
            hint,
            &assignment_ids,
            context.lowered,
            &guard.span,
        )?;
        let TopologyAssignmentRef::Generated(assignment) = &guard.edge.assignment else {
            return Err(TopologyHintError::new(
                guard.span.clone(),
                "routing guard must name a generated assignment edge",
            ));
        };
        if !rewrite_cones.values().any(|cone| cone.contains(assignment)) {
            return Err(TopologyHintError::new(
                guard.span.clone(),
                "routing guard edge is outside every rewrite replacement cone",
            ));
        }
    }

    let mut covered_components = BTreeSet::new();
    let mut variants = BTreeSet::new();
    let mut constraint_paths = BTreeMap::new();
    for recipe in &hint.path_recipes {
        if recipe.steps.is_empty() {
            return Err(TopologyHintError::new(
                recipe.span.clone(),
                format!(
                    "{} must contain at least one typed dependency step",
                    recipe.id
                ),
            ));
        }
        let constraint = resolve_constraint(&recipe.key, context.graph, &recipe.span)?;
        validate_recipe_steps(recipe, hint, &assignment_ids, context.lowered, &recipe.span)?;
        let Some(TopologyAssignmentRef::Rewrite(baseline)) =
            recipe.steps.last().map(|step| &step.assignment)
        else {
            return Err(TopologyHintError::new(
                recipe.span.clone(),
                format!("{} must terminate at a virtual rewrite step", recipe.id),
            ));
        };
        let rewrite = hint
            .rewrites
            .iter()
            .find(|rewrite| rewrite.baseline == *baseline)
            .ok_or_else(|| {
                TopologyHintError::new(
                    recipe.span.clone(),
                    format!("{} references missing rewrite for {baseline}", recipe.id),
                )
            })?;
        let expected_guards = BTreeSet::from([
            rewrite.knownness_guard.clone(),
            rewrite.exact_fallback_guard.clone(),
        ]);
        let mut omitted_guards = BTreeSet::new();
        for guard_id in &recipe.omitted_routing_guards {
            if !omitted_guards.insert(guard_id.clone()) {
                return Err(TopologyHintError::new(
                    recipe.span.clone(),
                    format!("{} omits duplicate routing guard {}", recipe.id, guard_id),
                ));
            }
        }
        if omitted_guards != expected_guards {
            return Err(TopologyHintError::new(
                recipe.span.clone(),
                format!(
                    "{} must omit exactly the rewrite knownness and exact-fallback guards",
                    recipe.id
                ),
            ));
        }
        for guard_id in &recipe.omitted_routing_guards {
            let Some(_guard) = guards.get(guard_id) else {
                return Err(TopologyHintError::new(
                    recipe.span.clone(),
                    format!("{} omits missing {}", recipe.id, guard_id),
                ));
            };
            // A guard is deliberately excluded from the timed physical walk.
            // `validate_dependency_edge` proves that it is a real typed edge
            // in the declared overlay/baseline cone; it must not appear here.
        }
        validate_recipe_delay(recipe, constraint, context.lowered, &recipe.span)?;
        let variant = format!(
            "{:?}:{:?}:{:?}",
            recipe.key, recipe.target_transition, recipe.steps
        );
        if !variants.insert(variant) {
            return Err(TopologyHintError::new(
                recipe.span.clone(),
                format!("duplicate identical {}", recipe.id),
            ));
        }
        covered_components.insert((recipe.key.clone(), recipe.target_transition));
        constraint_paths.insert(recipe.key.clone(), constraint.path_order());
    }
    validate_recipe_coverage(hint, context.graph, &covered_components, &hint.span)?;
    let used_guards = hint
        .path_recipes
        .iter()
        .flat_map(|recipe| recipe.omitted_routing_guards.iter())
        .chain(
            hint.rewrites
                .iter()
                .flat_map(|rewrite| [&rewrite.knownness_guard, &rewrite.exact_fallback_guard]),
        )
        .collect::<BTreeSet<_>>();
    for guard in &hint.routing_guards {
        if !used_guards.contains(&guard.id) {
            return Err(TopologyHintError::new(
                guard.span.clone(),
                format!("unused routing guard {}", guard.id),
            ));
        }
    }

    let guards = hint
        .routing_guards
        .iter()
        .map(|guard| match &guard.edge.assignment {
            TopologyAssignmentRef::Generated(assignment) => Ok(ResolvedRoutingGuard {
                id: guard.id.clone(),
                assignment: assignment.clone(),
                operand_index: guard.edge.operand_index,
                reason: guard.reason,
            }),
            _ => Err(TopologyHintError::new(
                guard.span.clone(),
                "routing guard must bind a generated assignment",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rewrites = hint
        .rewrites
        .iter()
        .map(|rewrite| ResolvedTopologyRewrite {
            baseline: rewrite.baseline.clone(),
            replacement: rewrite.replacement.clone(),
            fallback: rewrite.fallback.clone(),
            knownness_guard: rewrite.knownness_guard.clone(),
            exact_fallback_guard: rewrite.exact_fallback_guard.clone(),
            span: rewrite.span.clone(),
        })
        .collect();
    let recipes = hint
        .path_recipes
        .iter()
        .map(|recipe| {
            Ok(ResolvedPathRecipe {
                id: recipe.id.clone(),
                span: recipe.span.clone(),
                path_order: recipe.key.path_order,
                control_order: recipe.key.control_order,
                target: recipe.key.target.clone(),
                transition: recipe.target_transition,
                ingress: resolve_recipe_ingress(recipe, hint)?,
                expected_terms: recipe.expected_terms.clone(),
                expected: delay_component_expr(
                    &recipe.expected_terms,
                    context.lowered,
                    &recipe.span,
                )?,
                steps: recipe
                    .steps
                    .iter()
                    .map(|step| ResolvedPathStep {
                        kind: match &step.assignment {
                            TopologyAssignmentRef::BaselineId(id) => {
                                ResolvedPathStepKind::Baseline(id.clone())
                            }
                            TopologyAssignmentRef::Generated(id) => {
                                ResolvedPathStepKind::Generated(id.clone())
                            }
                            TopologyAssignmentRef::Rewrite(id) => {
                                ResolvedPathStepKind::Rewrite(id.clone())
                            }
                        },
                        operand_index: step.operand_index,
                        transition: step.transition,
                    })
                    .collect(),
                omitted_guards: recipe.omitted_routing_guards.clone(),
            })
        })
        .collect::<Result<Vec<_>, TopologyHintError>>()?;

    Ok(ResolvedTopologyHint {
        hint: hint.clone(),
        signals: hint
            .signals
            .iter()
            .map(|signal| ResolvedTopologySignal {
                id: signal.id.clone(),
                name: signal.name.clone(),
                span: signal.span.clone(),
            })
            .collect(),
        assignments,
        baseline_assignments: resolved_baselines,
        guards,
        rewrites,
        recipes,
        alias_terms,
        constraint_paths,
    })
}

fn resolve_recipe_ingress(
    recipe: &TopologyPathRecipe,
    hint: &TopologyHint,
) -> Result<ResolvedRecipeIngress, TopologyHintError> {
    let first = recipe.steps.first().ok_or_else(|| {
        TopologyHintError::new(
            recipe.span.clone(),
            "path recipe has no first step for ingress",
        )
    })?;
    let expression = match &first.assignment {
        TopologyAssignmentRef::Generated(id) => {
            &hint
                .assignments
                .iter()
                .find(|assignment| assignment.id == *id)
                .ok_or_else(|| {
                    TopologyHintError::new(
                        recipe.span.clone(),
                        format!("ingress references missing {id}"),
                    )
                })?
                .expression
        }
        TopologyAssignmentRef::BaselineId(id) => {
            &hint
                .baseline_assignments
                .iter()
                .find(|assignment| assignment.id == *id)
                .ok_or_else(|| {
                    TopologyHintError::new(
                        recipe.span.clone(),
                        format!("ingress references missing {id}"),
                    )
                })?
                .anchor
                .expression
        }
        TopologyAssignmentRef::Rewrite(id) => {
            return Err(TopologyHintError::new(
                recipe.span.clone(),
                format!("path recipe cannot enter through rewrite {id}"),
            ));
        }
    };
    let operand = expression
        .operands()
        .get(first.operand_index)
        .ok_or_else(|| {
            TopologyHintError::new(recipe.span.clone(), "ingress operand index is invalid")
        })?;
    if operand == recipe.key.control() {
        return Ok(ResolvedRecipeIngress::DirectControl);
    }
    let baseline = hint
        .baseline_assignments
        .iter()
        .find(|baseline| {
            baseline.anchor.target == *operand
                && baseline.anchor.expression
                    == TopologyValueExpr::Atom(recipe.key.control().to_string())
        })
        .ok_or_else(|| {
            TopologyHintError::new(
                recipe.span.clone(),
                "path recipe ingress is neither direct control nor an anchored baseline buffer",
            )
        })?;
    Ok(ResolvedRecipeIngress::BaselineBuffer(baseline.id.clone()))
}

fn resolve_baseline_assignments(
    hint: &TopologyHint,
    lowered: &LoweredModule,
) -> Result<Vec<ResolvedBaselineAssignment>, TopologyHintError> {
    let mut resolved = Vec::with_capacity(hint.baseline_assignments.len());
    let mut bound_orders = BTreeSet::new();
    for baseline in &hint.baseline_assignments {
        let expected = baseline.anchor.expression.to_expr();
        let matches = lowered
            .cell
            .items
            .iter()
            .enumerate()
            .filter_map(|(item_order, item)| match item {
                CellItem::Assignment(assignment)
                    if assignment.target == baseline.anchor.target
                        && assignment.expr == expected =>
                {
                    Some((item_order, assignment))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(item_order, _)] = matches.as_slice() else {
            return Err(TopologyHintError::new(
                baseline.span.clone(),
                format!(
                    "baseline assignment `{}` does not identify exactly one lowered assignment",
                    baseline.id
                ),
            ));
        };
        let assignment_order = lowered.cell.items[..=*item_order]
            .iter()
            .filter(|item| matches!(item, CellItem::Assignment(_)))
            .count()
            - 1;
        if !bound_orders.insert(assignment_order) {
            return Err(TopologyHintError::new(
                baseline.span.clone(),
                format!(
                    "baseline assignment `{}` duplicates an existing structural anchor",
                    baseline.id
                ),
            ));
        }
        resolved.push(ResolvedBaselineAssignment {
            id: baseline.id.clone(),
            anchor: baseline.anchor.clone(),
            item_order: *item_order,
            assignment_order,
            span: baseline.span.clone(),
        });
    }
    Ok(resolved)
}

fn validate_anchor(
    anchor: &BaselineAssignmentAnchor,
    lowered: &LoweredModule,
    must_be_register: bool,
    span: &Span,
    label: &str,
) -> Result<(), TopologyHintError> {
    if must_be_register
        && !lowered
            .cell
            .registers
            .iter()
            .any(|register| register.name == anchor.target)
    {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "baseline {label} anchor `{}` is not a modeled register",
                anchor.target
            ),
        ));
    }
    let expected = anchor.expression.to_expr();
    let count = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(value) => Some(value),
            _ => None,
        })
        .filter(|assignment| assignment.target == anchor.target && assignment.expr == expected)
        .count();
    if count != 1 {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "baseline {label} anchor `{}` does not identify exactly one assignment",
                anchor.target
            ),
        ));
    }
    Ok(())
}

fn validate_overlay_expression(
    expression: &TopologyValueExpr,
    baseline: &BTreeSet<String>,
    signals: &BTreeMap<HintSignalId, String>,
    span: &Span,
) -> Result<(), TopologyHintError> {
    for operand in expression.operands() {
        if !baseline.contains(operand)
            && !signals.values().any(|name| name == operand)
            && !is_logic_atom(operand)
        {
            return Err(TopologyHintError::new(
                span.clone(),
                format!("overlay expression references unknown atom `{operand}`"),
            ));
        }
    }
    Ok(())
}

fn validate_recipe_steps(
    recipe: &TopologyPathRecipe,
    hint: &TopologyHint,
    assignment_ids: &BTreeSet<HintAssignmentId>,
    lowered: &LoweredModule,
    span: &Span,
) -> Result<(), TopologyHintError> {
    let mut expected_input = recipe.key.control().to_string();
    let mut input_transition = None;
    for (step_index, step) in recipe.steps.iter().enumerate() {
        let (target, expression) = match &step.assignment {
            TopologyAssignmentRef::Generated(id) => {
                if !assignment_ids.contains(id) {
                    return Err(TopologyHintError::new(
                        span.clone(),
                        format!("{} references missing {id}", recipe.id),
                    ));
                }
                let assignment = hint
                    .assignments
                    .iter()
                    .find(|assignment| assignment.id == *id)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!("{} references missing {id}", recipe.id),
                        )
                    })?;
                let target = hint
                    .signals
                    .iter()
                    .find(|signal| signal.id == assignment.target)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!(
                                "{} targets missing signal {}",
                                assignment.id, assignment.target
                            ),
                        )
                    })?
                    .name
                    .clone();
                (target, assignment.expression.clone())
            }
            TopologyAssignmentRef::Rewrite(id) => {
                let rewrite = hint
                    .rewrites
                    .iter()
                    .find(|rewrite| rewrite.baseline == *id)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!("{} references missing rewrite for {id}", recipe.id),
                        )
                    })?;
                let replacement = hint
                    .assignments
                    .iter()
                    .find(|assignment| assignment.id == rewrite.replacement)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!(
                                "rewrite for {id} references missing {}",
                                rewrite.replacement
                            ),
                        )
                    })?;
                let replacement_target = hint
                    .signals
                    .iter()
                    .find(|signal| signal.id == replacement.target)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!(
                                "{} targets missing signal {}",
                                replacement.id, replacement.target
                            ),
                        )
                    })?
                    .name
                    .clone();
                (
                    rewrite.anchor.target.clone(),
                    TopologyValueExpr::Atom(replacement_target),
                )
            }
            TopologyAssignmentRef::BaselineId(id) => {
                let assignment = hint
                    .baseline_assignments
                    .iter()
                    .find(|assignment| assignment.id == *id)
                    .ok_or_else(|| {
                        TopologyHintError::new(
                            span.clone(),
                            format!("{} references missing {id}", recipe.id),
                        )
                    })?;
                validate_anchor(
                    &assignment.anchor,
                    lowered,
                    assignment.anchor == hint.baseline.state,
                    &assignment.span,
                    "step",
                )?;
                (
                    assignment.anchor.target.clone(),
                    assignment.anchor.expression.clone(),
                )
            }
        };
        let operand = expression
            .operands()
            .get(step.operand_index)
            .ok_or_else(|| {
                TopologyHintError::new(
                    span.clone(),
                    format!(
                        "{} operand_index {} is invalid for assignment target `{target}`",
                        recipe.id, step.operand_index
                    ),
                )
            })?;
        // A retained specify control can enter the physical overlay through a
        // zero-delay baseline buffer. The buffer remains an independently
        // anchored baseline assignment, while the first generated step names
        // its buffered output directly. This avoids making a no-delay
        // forwarding edge look like a generated timing placement.
        let enters_through_baseline_buffer = step_index == 0
            && hint.baseline_assignments.iter().any(|baseline| {
                baseline.anchor.target == *operand
                    && baseline.anchor.expression == TopologyValueExpr::Atom(expected_input.clone())
            });
        if operand != &expected_input && !enters_through_baseline_buffer {
            return Err(TopologyHintError::new(
                span.clone(),
                format!(
                    "{} has discontinuous dependency walk: `{target}` consumes `{operand}`, expected `{expected_input}`",
                    recipe.id
                ),
            ));
        }
        if let Some(input_transition) = input_transition
            && let Some(expected_transition) =
                unate_output_transition(&expression, step.operand_index, input_transition)
            && step.transition != expected_transition
        {
            return Err(TopologyHintError::new(
                span.clone(),
                format!(
                    "{} has transition-inconsistent unate edge into {} operand {}: {:?} input requires {:?} output, not {:?}",
                    recipe.id,
                    target,
                    step.operand_index,
                    input_transition,
                    expected_transition,
                    step.transition
                ),
            ));
        }
        expected_input = target;
        input_transition = Some(step.transition);
    }
    if expected_input != recipe.key.target {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "{} ends at `{expected_input}`, not retained target `{}`",
                recipe.id, recipe.key.target
            ),
        ));
    }
    Ok(())
}

/// Returns the output transition forced by one locally unate operand. Mux
/// selectors and every conditional/non-unate primitive intentionally return
/// None: their sensitization remains explicitly documented by the recipe.
fn unate_output_transition(
    expression: &TopologyValueExpr,
    operand_index: usize,
    input: Transition,
) -> Option<Transition> {
    if input == Transition::TurnOff {
        return None;
    }
    let flip = |transition| match transition {
        Transition::Rise => Transition::Fall,
        Transition::Fall => Transition::Rise,
        Transition::TurnOff => unreachable!(),
    };
    match expression {
        TopologyValueExpr::Atom(_) => Some(input),
        TopologyValueExpr::Operation {
            operator: ValueOperator::Not | ValueOperator::Nand | ValueOperator::Nor,
            ..
        } => Some(flip(input)),
        TopologyValueExpr::Operation {
            operator: ValueOperator::And | ValueOperator::Or,
            ..
        } => Some(input),
        TopologyValueExpr::Operation {
            operator: ValueOperator::BufIf0 | ValueOperator::BufIf1,
            ..
        } if operand_index == 0 => Some(input),
        TopologyValueExpr::Operation {
            operator: ValueOperator::Mux,
            ..
        } if operand_index > 0 => Some(input),
        TopologyValueExpr::Operation { .. } => None,
    }
}

fn generated_cone(
    root: &HintAssignmentId,
    hint: &TopologyHint,
    signals: &BTreeMap<HintSignalId, String>,
) -> BTreeSet<HintAssignmentId> {
    let producers = hint
        .assignments
        .iter()
        .filter_map(|assignment| {
            signals
                .get(&assignment.target)
                .map(|signal| (signal, &assignment.id))
        })
        .collect::<BTreeMap<_, _>>();
    let mut cone = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(id) = pending.pop() {
        if !cone.insert(id.clone()) {
            continue;
        }
        let Some(assignment) = hint
            .assignments
            .iter()
            .find(|assignment| assignment.id == id)
        else {
            continue;
        };
        for operand in assignment.expression.operands() {
            if let Some(producer) = producers.get(operand) {
                pending.push((*producer).clone());
            }
        }
    }
    cone
}

fn validate_dependency_edge(
    edge: &TopologyDependencyEdge,
    hint: &TopologyHint,
    assignment_ids: &BTreeSet<HintAssignmentId>,
    lowered: &LoweredModule,
    span: &Span,
) -> Result<(), TopologyHintError> {
    let expression = match &edge.assignment {
        TopologyAssignmentRef::Rewrite(_) => {
            return Err(TopologyHintError::new(
                span.clone(),
                "routing guards may not name a virtual rewrite edge",
            ));
        }
        TopologyAssignmentRef::Generated(id) => {
            if !assignment_ids.contains(id) {
                return Err(TopologyHintError::new(
                    span.clone(),
                    format!("guard references missing {id}"),
                ));
            }
            &hint
                .assignments
                .iter()
                .find(|assignment| assignment.id == *id)
                .ok_or_else(|| {
                    TopologyHintError::new(span.clone(), format!("guard references missing {id}"))
                })?
                .expression
        }
        TopologyAssignmentRef::BaselineId(id) => {
            let assignment = hint
                .baseline_assignments
                .iter()
                .find(|assignment| assignment.id == *id)
                .ok_or_else(|| {
                    TopologyHintError::new(span.clone(), format!("guard references missing {id}"))
                })?;
            validate_anchor(
                &assignment.anchor,
                lowered,
                assignment.anchor == hint.baseline.state,
                &assignment.span,
                "guard",
            )?;
            &assignment.anchor.expression
        }
    };
    if expression.operands().get(edge.operand_index).is_none() {
        return Err(TopologyHintError::new(
            span.clone(),
            format!("guard operand_index {} is invalid", edge.operand_index),
        ));
    }
    Ok(())
}

fn resolve_delay_terms(
    tuple: &TopologyDelayTuple,
    lowered: &LoweredModule,
    output: &mut BTreeMap<String, TimingExpr>,
    span: &Span,
) -> Result<(), TopologyHintError> {
    for component in tuple.components() {
        for term in component.terms() {
            let Some(value) = lowered.timing_aliases.get(term) else {
                return Err(TopologyHintError::new(
                    span.clone(),
                    format!("timing term `{term}` is not a resolved alias or specparam"),
                ));
            };
            output.insert(term.clone(), value.clone());
        }
    }
    Ok(())
}

fn resolve_topology_delay(
    tuple: &TopologyDelayTuple,
    lowered: &LoweredModule,
    span: &Span,
) -> Result<DelayTuple, TopologyHintError> {
    match tuple {
        TopologyDelayTuple::One(one) => {
            Ok(DelayTuple::One(delay_component_expr(one, lowered, span)?))
        }
        TopologyDelayTuple::Two { rise, fall } => Ok(DelayTuple::Two {
            rise: delay_component_expr(rise, lowered, span)?,
            fall: delay_component_expr(fall, lowered, span)?,
        }),
        TopologyDelayTuple::Three {
            rise,
            fall,
            turn_off,
        } => Ok(DelayTuple::Three {
            rise: delay_component_expr(rise, lowered, span)?,
            fall: delay_component_expr(fall, lowered, span)?,
            turn_off: delay_component_expr(turn_off, lowered, span)?,
        }),
    }
}

fn resolve_constraint<'a>(
    key: &TopologyConstraintKey,
    graph: &'a TimingGraph,
    span: &Span,
) -> Result<&'a TimingConstraint, TopologyHintError> {
    let Some(constraint) = graph.constraints().get(key.path_order) else {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "no retained timing constraint at path_order {}",
                key.path_order
            ),
        ));
    };
    let Some(control) = constraint.controls().get(key.control_order) else {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "retained timing constraint {} has no control_order {}",
                key.path_order, key.control_order
            ),
        ));
    };
    if constraint.target() != key.target || control.source().signal() != key.control {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "stale retained constraint key at path_order {}",
                key.path_order
            ),
        ));
    }
    Ok(constraint)
}

fn validate_recipe_delay(
    recipe: &TopologyPathRecipe,
    constraint: &TimingConstraint,
    lowered: &LoweredModule,
    span: &Span,
) -> Result<(), TopologyHintError> {
    let index = transition_component(recipe.target_transition, constraint.delay().len())
        .ok_or_else(|| {
            TopologyHintError::new(
                span.clone(),
                format!(
                    "{} selects unsupported {:?} component of a {}-entry delay tuple",
                    recipe.id,
                    recipe.target_transition,
                    constraint.delay().len()
                ),
            )
        })?;
    let expected = delay_component_expr(&recipe.expected_terms, lowered, span)?;
    let actual = constraint
        .additive_delay()
        .component(index)
        .ok_or_else(|| {
            TopologyHintError::new(span.clone(), "selected delay component is absent")
        })?;
    let expected = crate::timing_terms::AdditiveDelay::from_timing_expr(expected)
        .map_err(|error| TopologyHintError::new(span.clone(), error.to_string()))?;
    // Addition nesting in the parsed specify source is not semantically part
    // of a path term list. Compare the flattened, source-ordered opaque terms
    // so a checked-in n-ary TOML list exactly describes the retained delay
    // component without depending on parser association.
    if expected.terms() != actual.terms() {
        return Err(TopologyHintError::new(
            span.clone(),
            format!(
                "{} expected terms do not exactly match its selected retained tuple component",
                recipe.id
            ),
        ));
    }
    Ok(())
}

fn transition_component(transition: Transition, tuple_len: usize) -> Option<usize> {
    match (transition, tuple_len) {
        (Transition::Rise, 1..=3) => Some(0),
        (Transition::Fall, 2..=3) => Some(1),
        (Transition::TurnOff, 3) => Some(2),
        _ => None,
    }
}

fn delay_component_expr(
    terms: &DelayComponentTerms,
    lowered: &LoweredModule,
    span: &Span,
) -> Result<TimingExpr, TopologyHintError> {
    let expressions = terms
        .terms()
        .iter()
        .map(|term| {
            lowered.timing_aliases.get(term).cloned().ok_or_else(|| {
                TopologyHintError::new(
                    span.clone(),
                    format!("timing term `{term}` is not a resolved alias or specparam"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    match expressions.as_slice() {
        [] => TimingExpr::atom("0")
            .map_err(|error| TopologyHintError::new(span.clone(), error.to_string())),
        [value] => Ok(value.clone()),
        _ => TimingExpr::operation(crate::ir::TimingOperator::Add, expressions)
            .map_err(|error| TopologyHintError::new(span.clone(), error.to_string())),
    }
}

fn validate_recipe_coverage(
    hint: &TopologyHint,
    graph: &TimingGraph,
    covered: &BTreeSet<(TopologyConstraintKey, Transition)>,
    span: &Span,
) -> Result<(), TopologyHintError> {
    let targets = hint
        .baseline
        .outputs
        .iter()
        .map(|anchor| anchor.target.as_str())
        .chain(std::iter::once(hint.baseline.state.target.as_str()))
        .collect::<BTreeSet<_>>();
    for constraint in graph
        .constraints()
        .iter()
        .filter(|constraint| targets.contains(constraint.target()))
    {
        for control in constraint.controls() {
            let key = TopologyConstraintKey {
                path_order: constraint.path_order(),
                control_order: control.order_in_path(),
                control: control.source().signal().to_string(),
                target: constraint.target().to_string(),
            };
            for index in 0..constraint.delay().len() {
                let transition = match index {
                    0 => Transition::Rise,
                    1 => Transition::Fall,
                    2 => Transition::TurnOff,
                    _ => unreachable!(),
                };
                if !covered.contains(&(key.clone(), transition)) {
                    return Err(TopologyHintError::new(
                        span.clone(),
                        format!(
                            "missing path recipe for retained path {} control {} target `{}` {:?}",
                            key.path_order, key.control_order, key.target, transition
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn collect_baseline_names(lowered: &LoweredModule) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    names.extend(lowered.cell.inputs.iter().cloned());
    names.extend(lowered.cell.outputs.iter().cloned());
    names.extend(
        lowered
            .cell
            .registers
            .iter()
            .map(|register| register.name.clone()),
    );
    for item in &lowered.cell.items {
        if let CellItem::Assignment(assignment) = item {
            names.insert(assignment.target.clone());
        }
    }
    names
}

fn is_reserved_timing_name(name: &str) -> bool {
    let Some((prefix, digits)) = name.split_at_checked(1) else {
        return false;
    };
    matches!(prefix, "t" | "d")
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_logic_atom(value: &str) -> bool {
    matches!(value, "0" | "1" | "x" | "z")
}

fn logic_atom(value: &str) -> Option<crate::ir::LogicValue> {
    match value {
        "0" => Some(crate::ir::LogicValue::Zero),
        "1" => Some(crate::ir::LogicValue::One),
        "x" => Some(crate::ir::LogicValue::X),
        "z" => Some(crate::ir::LogicValue::Z),
        _ => None,
    }
}

fn atom(value: String, label: &str, span: &Span) -> Result<String, TopologyHintError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(TopologyHintError::new(
            span.clone(),
            format!("{label} must be a non-empty atom"),
        ));
    }
    Ok(value)
}

fn id(value: String, label: &str, span: &Span) -> Result<String, TopologyHintError> {
    atom(value, label, span)
}

fn parse_mode(value: &str, span: &Span) -> Result<GenerateMode, TopologyHintError> {
    match value {
        "delayful" => Ok(GenerateMode::Delayful),
        "nodelay" => Ok(GenerateMode::Nodelay),
        _ => Err(TopologyHintError::new(
            span.clone(),
            format!("unknown generate_mode `{value}`; expected `delayful` or `nodelay`"),
        )),
    }
}

fn parse_transition(value: String, span: &Span) -> Result<Transition, TopologyHintError> {
    match value.as_str() {
        "rise" => Ok(Transition::Rise),
        "fall" => Ok(Transition::Fall),
        "turn-off" => Ok(Transition::TurnOff),
        _ => Err(TopologyHintError::new(
            span.clone(),
            format!("unknown transition `{value}`; expected `rise`, `fall`, or `turn-off`"),
        )),
    }
}

fn parse_guard_reason(value: &str, span: &Span) -> Result<RoutingGuardReason, TopologyHintError> {
    match value {
        "routing" => Ok(RoutingGuardReason::Routing),
        "knownness" => Ok(RoutingGuardReason::Knownness),
        "exact-fallback" => Ok(RoutingGuardReason::ExactFallback),
        _ => Err(TopologyHintError::new(
            span.clone(),
            format!("unknown routing guard reason `{value}`"),
        )),
    }
}

fn span_at_offset(path: &Path, input: &str, offset: usize) -> Span {
    let offset = offset.min(input.len());
    let before = &input[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    Span::new(path, line, column)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    hints: Vec<toml::Spanned<RawHint>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHint {
    module: String,
    generate_mode: String,
    baseline: toml::Spanned<RawBaseline>,
    #[serde(default)]
    baseline_assignments: Vec<toml::Spanned<RawBaselineAssignment>>,
    #[serde(default)]
    signals: Vec<toml::Spanned<RawSignal>>,
    #[serde(default)]
    assignments: Vec<toml::Spanned<RawAssignment>>,
    #[serde(default)]
    routing_guards: Vec<toml::Spanned<RawRoutingGuard>>,
    #[serde(default)]
    path_recipes: Vec<toml::Spanned<RawPathRecipe>>,
    #[serde(default)]
    rewrites: Vec<toml::Spanned<RawRewrite>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBaseline {
    state: toml::Spanned<RawAnchor>,
    outputs: Vec<toml::Spanned<RawAnchor>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnchor {
    target: String,
    expression: RawExpression,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBaselineAssignment {
    id: String,
    target: String,
    expression: RawExpression,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRewrite {
    anchor_id: String,
    replacement: String,
    fallback: String,
    knownness_guard: String,
    exact_fallback_guard: String,
}

impl RawAnchor {
    fn into_anchor(self, span: &Span) -> Result<BaselineAssignmentAnchor, TopologyHintError> {
        Ok(BaselineAssignmentAnchor {
            target: atom(self.target, "baseline target", span)?,
            expression: self.expression.into_expression(span)?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSignal {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssignment {
    id: String,
    target: String,
    expression: RawExpression,
    delay: RawDelayTuple,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpression {
    atom: Option<String>,
    operator: Option<String>,
    #[serde(default)]
    operands: Vec<String>,
}

impl RawExpression {
    fn into_expression(self, span: &Span) -> Result<TopologyValueExpr, TopologyHintError> {
        match (self.atom, self.operator) {
            (Some(value), None) if self.operands.is_empty() => Ok(TopologyValueExpr::Atom(atom(
                value,
                "expression atom",
                span,
            )?)),
            (None, Some(operator)) => {
                let operator = ValueOperator::parse(&operator).ok_or_else(|| {
                    TopologyHintError::new(
                        span.clone(),
                        format!("uncontracted value operator `{operator}`"),
                    )
                })?;
                let operands = self
                    .operands
                    .into_iter()
                    .map(|operand| atom(operand, "expression operand", span))
                    .collect::<Result<Vec<_>, _>>()?;
                if !operator.accepts_arity(operands.len()) {
                    return Err(TopologyHintError::new(
                        span.clone(),
                        format!(
                            "wrong arity for value operator `{}`: got {}",
                            operator.as_str(),
                            operands.len()
                        ),
                    ));
                }
                Ok(TopologyValueExpr::Operation { operator, operands })
            }
            _ => Err(TopologyHintError::new(
                span.clone(),
                "expression must contain exactly one of `atom` or `operator`, and operator operands must be flat atoms",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDelayTuple {
    one: Option<Vec<String>>,
    rise: Option<Vec<String>>,
    fall: Option<Vec<String>>,
    turn_off: Option<Vec<String>>,
}

impl RawDelayTuple {
    fn into_delay(self, span: &Span) -> Result<TopologyDelayTuple, TopologyHintError> {
        let terms = |values: Vec<String>| {
            values
                .into_iter()
                .map(|value| atom(value, "timing term", span))
                .collect::<Result<Vec<_>, _>>()
                .map(DelayComponentTerms)
        };
        match (self.one, self.rise, self.fall, self.turn_off) {
            (Some(one), None, None, None) => Ok(TopologyDelayTuple::One(terms(one)?)),
            (None, Some(rise), Some(fall), None) => Ok(TopologyDelayTuple::Two {
                rise: terms(rise)?,
                fall: terms(fall)?,
            }),
            (None, Some(rise), Some(fall), Some(turn_off)) => Ok(TopologyDelayTuple::Three {
                rise: terms(rise)?,
                fall: terms(fall)?,
                turn_off: terms(turn_off)?,
            }),
            _ => Err(TopologyHintError::new(
                span.clone(),
                "delay must be exactly one of `{ one = [...] }`, `{ rise = [...], fall = [...] }`, or `{ rise = [...], fall = [...], turn_off = [...] }`",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoutingGuard {
    id: String,
    assignment: RawStepAssignment,
    operand_index: usize,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPathRecipe {
    id: String,
    key: RawConstraintKey,
    target_transition: String,
    steps: Vec<RawPathStep>,
    expected_terms: Vec<String>,
    #[serde(default)]
    omitted_routing_guards: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPathStep {
    assignment: RawStepAssignment,
    operand_index: usize,
    transition: String,
}

impl RawPathStep {
    fn into_step(self, span: &Span) -> Result<TopologyPathStep, TopologyHintError> {
        Ok(TopologyPathStep {
            assignment: self.assignment.into_assignment_ref(span)?,
            operand_index: self.operand_index,
            transition: parse_transition(self.transition, span)?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStepAssignment {
    generated: Option<String>,
    baseline_id: Option<String>,
    rewrite: Option<String>,
}

impl RawStepAssignment {
    fn into_assignment_ref(self, span: &Span) -> Result<TopologyAssignmentRef, TopologyHintError> {
        match (self.generated, self.baseline_id, self.rewrite) {
            (Some(value), None, None) => Ok(TopologyAssignmentRef::Generated(HintAssignmentId(
                id(value, "step generated assignment", span)?,
            ))),
            (None, Some(value), None) => Ok(TopologyAssignmentRef::BaselineId(
                BaselineAssignmentId(id(value, "step baseline assignment", span)?),
            )),
            (None, None, Some(value)) => Ok(TopologyAssignmentRef::Rewrite(BaselineAssignmentId(
                id(value, "rewrite step", span)?,
            ))),
            _ => Err(TopologyHintError::new(
                span.clone(),
                "path step assignment must contain exactly one of `generated`, `baseline_id`, or `rewrite`",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConstraintKey {
    path_order: usize,
    control_order: usize,
    control: String,
    target: String,
}

impl RawConstraintKey {
    fn into_key(self, span: &Span) -> Result<TopologyConstraintKey, TopologyHintError> {
        Ok(TopologyConstraintKey {
            path_order: self.path_order,
            control_order: self.control_order,
            control: atom(self.control, "constraint control", span)?,
            target: atom(self.target, "constraint target", span)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Span;
    use crate::ir::{Assignment, Cell, DelayTuple, LogicValue, Register};
    use crate::timing_graph::{
        AssignmentDelayOrigin, AssignmentOrigin, AssignmentProvenance, SourceAssignmentOrigin,
        StateControlProvenance, TimingConstraintSource, TimingControlSource, TimingNodeKind,
        TimingSignalMetadata, TimingSignalRole, build_timing_graph,
    };

    const HINT: &str = r#"
[[hints]]
module = "gate"
generate_mode = "delayful"
baseline = { state = { target = "q", expression = { atom = "d" } }, outputs = [{ target = "y", expression = { atom = "clk" } }] }
baseline_assignments = [{ id = "state", target = "q", expression = { atom = "d" } }, { id = "output", target = "y", expression = { atom = "clk" } }]
signals = [{ id = "raw", name = "q_raw" }, { id = "fallback", name = "q_fallback" }, { id = "replacement", name = "q_replacement" }, { id = "known", name = "q_known" }, { id = "state_fallback", name = "q_state_fallback" }, { id = "state_replacement", name = "q_state_replacement" }, { id = "state_known", name = "q_state_known" }]
assignments = [{ id = "inv", target = "raw", expression = { operator = "not", operands = ["clk"] }, delay = { rise = ["TR"], fall = ["TF"] } }, { id = "fallback", target = "fallback", expression = { atom = "clk" }, delay = { one = [] } }, { id = "known", target = "known", expression = { atom = "1" }, delay = { one = [] } }, { id = "replacement", target = "replacement", expression = { operator = "mux", operands = ["q_known", "q_raw", "q_fallback"] }, delay = { one = [] } }, { id = "state_fallback", target = "state_fallback", expression = { atom = "d" }, delay = { one = [] } }, { id = "state_known", target = "state_known", expression = { atom = "1" }, delay = { one = [] } }, { id = "state_replacement", target = "state_replacement", expression = { operator = "mux", operands = ["q_state_known", "q_raw", "q_state_fallback"] }, delay = { one = [] } }]
routing_guards = [{ id = "known_guard", assignment = { generated = "replacement" }, operand_index = 0, reason = "knownness" }, { id = "fallback_guard", assignment = { generated = "replacement" }, operand_index = 2, reason = "exact-fallback" }, { id = "state_known_guard", assignment = { generated = "state_replacement" }, operand_index = 0, reason = "knownness" }, { id = "state_fallback_guard", assignment = { generated = "state_replacement" }, operand_index = 2, reason = "exact-fallback" }]
rewrites = [{ anchor_id = "state", replacement = "state_replacement", fallback = "state_fallback", knownness_guard = "state_known_guard", exact_fallback_guard = "state_fallback_guard" }, { anchor_id = "output", replacement = "replacement", fallback = "fallback", knownness_guard = "known_guard", exact_fallback_guard = "fallback_guard" }]
path_recipes = [
  { id = "clock-y-rise", key = { path_order = 0, control_order = 0, control = "clk", target = "y" }, target_transition = "rise", steps = [{ assignment = { generated = "inv" }, operand_index = 0, transition = "rise" }, { assignment = { generated = "replacement" }, operand_index = 1, transition = "rise" }, { assignment = { rewrite = "output" }, operand_index = 0, transition = "rise" }], expected_terms = ["TR"], omitted_routing_guards = ["known_guard", "fallback_guard"] },
  { id = "clock-y-fall", key = { path_order = 0, control_order = 0, control = "clk", target = "y" }, target_transition = "fall", steps = [{ assignment = { generated = "inv" }, operand_index = 0, transition = "fall" }, { assignment = { generated = "replacement" }, operand_index = 1, transition = "fall" }, { assignment = { rewrite = "output" }, operand_index = 0, transition = "fall" }], expected_terms = ["TF"], omitted_routing_guards = ["known_guard", "fallback_guard"] }
]
"#;

    fn context() -> (LoweredModule, TimingGraph) {
        let timing = |name| TimingExpr::atom(name).unwrap();
        let lowered = LoweredModule {
            cell: Cell {
                name: "gate".into(),
                inputs: vec!["d".into(), "clk".into()],
                outputs: vec!["y".into()],
                registers: vec![Register {
                    name: "q".into(),
                    initial: LogicValue::Zero,
                }],
                items: vec![
                    CellItem::Assignment(Assignment {
                        target: "q".into(),
                        expr: Expr::atom("d"),
                        delay: DelayTuple::One(TimingExpr::atom("0").unwrap()),
                    }),
                    CellItem::Assignment(Assignment {
                        target: "y".into(),
                        expr: Expr::atom("clk"),
                        delay: DelayTuple::One(TimingExpr::atom("0").unwrap()),
                    }),
                ],
            },
            timing_aliases: BTreeMap::from([
                ("TR".into(), timing("TR")),
                ("TF".into(), timing("TF")),
            ]),
            diagnostics: vec![],
        };
        let span = Span::new("gate.sv", 1, 1);
        let metadata = vec![
            TimingSignalMetadata::new(
                "d".into(),
                BTreeSet::from([TimingSignalRole::Input]),
                span.clone(),
            )
            .unwrap(),
            TimingSignalMetadata::new(
                "clk".into(),
                BTreeSet::from([TimingSignalRole::Input]),
                span.clone(),
            )
            .unwrap(),
            TimingSignalMetadata::new(
                "q".into(),
                BTreeSet::from([TimingSignalRole::ModeledRegister]),
                span.clone(),
            )
            .unwrap(),
            TimingSignalMetadata::new(
                "y".into(),
                BTreeSet::from([TimingSignalRole::Output]),
                span.clone(),
            )
            .unwrap(),
        ];
        let provenance = vec![
            crate::timing_graph::AssignmentProvenance::new(
                0,
                0,
                span.clone(),
                AssignmentOrigin::Source(SourceAssignmentOrigin::ProceduralStateful),
                vec![],
            )
            .unwrap(),
            crate::timing_graph::AssignmentProvenance::new(
                1,
                1,
                span.clone(),
                AssignmentOrigin::Source(SourceAssignmentOrigin::Continuous),
                vec![],
            )
            .unwrap(),
        ];
        let source = TimingConstraintSource::new(
            0,
            vec![TimingControlSource::new("clk", None, span.clone()).unwrap()],
            "y",
            DelayTuple::Two {
                rise: timing("TR"),
                fall: timing("TF"),
            },
            span,
        )
        .unwrap();
        (
            lowered.clone(),
            build_timing_graph(&lowered.cell, &metadata, &provenance, &[source]).unwrap(),
        )
    }

    fn materialization_inputs() -> (
        LoweredModule,
        TimingGraph,
        Vec<TimingSignalMetadata>,
        Vec<AssignmentProvenance>,
    ) {
        let (lowered, graph) = context();
        let span = Span::new("gate.sv", 1, 1);
        let metadata = [
            ("d", TimingSignalRole::Input),
            ("clk", TimingSignalRole::Input),
            ("q", TimingSignalRole::ModeledRegister),
            ("y", TimingSignalRole::Output),
        ]
        .into_iter()
        .map(|(name, role)| {
            TimingSignalMetadata::new(name.into(), BTreeSet::from([role]), span.clone()).unwrap()
        })
        .collect();
        let provenance = vec![
            AssignmentProvenance::new(
                0,
                0,
                span.clone(),
                AssignmentOrigin::Source(SourceAssignmentOrigin::ProceduralStateful),
                vec![StateControlProvenance::new(
                    "clk".into(),
                    None,
                    span.clone(),
                )],
            )
            .unwrap(),
            AssignmentProvenance::new(
                1,
                1,
                span,
                AssignmentOrigin::Source(SourceAssignmentOrigin::Continuous),
                vec![],
            )
            .unwrap(),
        ];
        (lowered, graph, metadata, provenance)
    }

    #[test]
    fn parses_and_resolves_deterministically() {
        let catalog = TopologyHintCatalog::parse("hint.toml", HINT).unwrap();
        let (lowered, graph) = context();
        let first = catalog
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        let second = catalog
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.hints()[0].require_materialization().hint().module(),
            "gate"
        );
        let hint = &first.hints()[0];
        assert_eq!(hint.baseline_assignments()[0].assignment_order(), 0);
        let assignment = hint.assignment(&HintAssignmentId("inv".into())).unwrap();
        assert_eq!(assignment.target_name(), "q_raw");
        assert_eq!(
            assignment.operands(),
            &[TopologyOperandRef::BaselineSignal("clk".into())]
        );
        assert_eq!(
            hint.guard(&RoutingGuardId("known_guard".into()))
                .unwrap()
                .operand_index,
            0
        );
        assert_eq!(
            hint.rewrite(&BaselineAssignmentId("output".into()))
                .unwrap()
                .replacement,
            HintAssignmentId("replacement".into())
        );
        let recipe = hint
            .recipe(&HintPathRecipeId("clock-y-rise".into()))
            .unwrap();
        assert_eq!(
            recipe.expected.as_expr(),
            &TimingExpr::atom("TR").unwrap().as_expr().clone()
        );
        assert!(matches!(
            recipe.steps.last().unwrap().kind,
            ResolvedPathStepKind::Rewrite(_)
        ));
    }

    #[test]
    fn materializes_and_erases_resolved_topology_exactly() {
        use crate::topology_apply::materialize_topology;

        let (lowered, graph, metadata, provenance) = materialization_inputs();
        let resolved = TopologyHintCatalog::parse("hint.toml", HINT)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        let first = materialize_topology(
            resolved.hints()[0].require_materialization(),
            &lowered,
            &metadata,
            &provenance,
        )
        .unwrap();
        let second = materialize_topology(
            resolved.hints()[0].require_materialization(),
            &lowered,
            &metadata,
            &provenance,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.facts.assignments.len(), 7);
        assert_eq!(first.facts.rewrites.len(), 2);
        assert_eq!(first.facts.original_assignment_orders.len(), 2);
        assert_eq!(first.provenance.len(), 9);
        assert_eq!(first.metadata.len(), 11);
        assert_eq!(&first.metadata[..metadata.len()], metadata.as_slice());
        assert!(
            first.metadata[4]
                .roles()
                .contains(&TimingSignalRole::TopologyTemporary)
        );
        assert!(first.provenance[0].origin().is_topology_generated());
        assert_eq!(
            first.provenance[0].delay_origin(),
            AssignmentDelayOrigin::TopologyPlacement
        );
        let assignments = first
            .lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(assignments[7].expr, Expr::atom("q_state_replacement"));
        assert_eq!(assignments[8].expr, Expr::atom("q_replacement"));
        for resolved_assignment in resolved.hints()[0].assignments() {
            let applied = first
                .facts
                .assignments
                .get(resolved_assignment.id())
                .unwrap();
            assert_eq!(
                applied.assignment.expr,
                resolved_assignment.expression().to_expr()
            );
            assert_eq!(applied.assignment.delay, *resolved_assignment.delay());
        }
        for (old_order, new_order) in &first.facts.original_assignment_orders {
            assert_eq!(
                first.provenance[*new_order].source_assignment_order(),
                provenance[*old_order].source_assignment_order()
            );
            assert_eq!(
                first.provenance[*new_order].origin(),
                provenance[*old_order].origin()
            );
            assert_eq!(
                first.provenance[*new_order].delay_origin(),
                provenance[*old_order].delay_origin()
            );
            assert_eq!(
                first.provenance[*new_order].span(),
                provenance[*old_order].span()
            );
            assert_eq!(
                first.provenance[*new_order].state_controls(),
                provenance[*old_order].state_controls()
            );
        }
        assert_eq!(first.lowered.cell.registers, lowered.cell.registers);
        assert_eq!(first.lowered.timing_aliases, lowered.timing_aliases);
        assert_eq!(first.lowered.diagnostics, lowered.diagnostics);
        let erased = first
            .erasure
            .erase(&first.lowered, &first.provenance, &first.metadata)
            .unwrap();
        assert_eq!(erased.0, lowered);
        assert_eq!(erased.1, provenance);
        assert_eq!(erased.2, metadata);

        let transformed_graph = build_timing_graph(
            &first.lowered.cell,
            &first.metadata,
            &first.provenance,
            &[TimingConstraintSource::new(
                0,
                vec![TimingControlSource::new("clk", None, Span::new("gate.sv", 1, 1)).unwrap()],
                "y",
                DelayTuple::Two {
                    rise: TimingExpr::atom("TR").unwrap(),
                    fall: TimingExpr::atom("TF").unwrap(),
                },
                Span::new("gate.sv", 1, 1),
            )
            .unwrap()],
        )
        .unwrap();
        let raw = transformed_graph
            .node(transformed_graph.signal_id("q_raw").unwrap())
            .unwrap();
        assert!(matches!(
            raw.kind(),
            TimingNodeKind::Signal(signal)
                if signal.has_role(TimingSignalRole::TopologyTemporary)
        ));

        let mut corrupt = first.lowered.clone();
        corrupt.cell.items.swap(0, 1);
        assert!(
            first
                .erasure
                .erase(&corrupt, &first.provenance, &first.metadata)
                .is_err()
        );
        let mut corrupt_provenance = first.provenance.clone();
        corrupt_provenance.swap(0, 1);
        assert!(
            first
                .erasure
                .erase(&first.lowered, &corrupt_provenance, &first.metadata)
                .is_err()
        );
        let mut corrupt_metadata = first.metadata.clone();
        corrupt_metadata.swap(0, 1);
        assert!(
            first
                .erasure
                .erase(&first.lowered, &first.provenance, &corrupt_metadata)
                .is_err()
        );
        let mut corrupt_register = first.lowered.clone();
        corrupt_register.cell.registers[0].initial = LogicValue::One;
        assert!(
            first
                .erasure
                .erase(&corrupt_register, &first.provenance, &first.metadata)
                .is_err()
        );
        let mut corrupt_delay = first.lowered.clone();
        if let CellItem::Assignment(assignment) = &mut corrupt_delay.cell.items[0] {
            assignment.delay = DelayTuple::One(TimingExpr::atom("0").unwrap());
        }
        assert!(
            first
                .erasure
                .erase(&corrupt_delay, &first.provenance, &first.metadata)
                .is_err()
        );
        let mut corrupt_rewrite = first.lowered.clone();
        if let CellItem::Assignment(assignment) = &mut corrupt_rewrite.cell.items[7] {
            assignment.expr = Expr::atom("d");
        }
        assert!(
            first
                .erasure
                .erase(&corrupt_rewrite, &first.provenance, &first.metadata)
                .is_err()
        );

        assert!(
            materialize_topology(
                resolved.hints()[0].require_materialization(),
                &lowered,
                &metadata,
                &provenance[..1],
            )
            .is_err()
        );
        let mut stale_metadata = metadata.clone();
        stale_metadata.push(
            TimingSignalMetadata::new(
                "stale".into(),
                BTreeSet::from([TimingSignalRole::Internal]),
                Span::new("gate.sv", 1, 1),
            )
            .unwrap(),
        );
        assert!(
            materialize_topology(
                resolved.hints()[0].require_materialization(),
                &lowered,
                &stale_metadata,
                &provenance,
            )
            .is_err()
        );
    }

    #[test]
    fn materialization_rejects_forward_and_dead_dependencies_with_item_spans() {
        use crate::topology_apply::materialize_topology;

        let cases = [
            (
                "ordinary baseline dependency after insertion",
                HINT.replace(
                    "{ id = \"known\", target = \"known\", expression = { atom = \"1\" }, delay = { one = [] } }",
                    "{ id = \"known\", target = \"known\", expression = { atom = \"y\" }, delay = { one = [] } }",
                ),
                "expression = { atom = \"y\" }",
                "after the topology insertion point",
            ),
            (
                "generated forward dependency",
                HINT.replace(
                    "{ id = \"known\", target = \"known\", expression = { atom = \"1\" }, delay = { one = [] } }",
                    "{ id = \"known\", target = \"known\", expression = { atom = \"q_replacement\" }, delay = { one = [] } }",
                ),
                "expression = { atom = \"q_replacement\" }",
                "forward dependency",
            ),
            (
                "dead generated assignment",
                HINT.replace(
                    "signals = [",
                    "signals = [{ id = \"dead\", name = \"q_dead\" }, ",
                )
                .replace(
                    "\nassignments = [",
                    "\nassignments = [{ id = \"dead\", target = \"dead\", expression = { atom = \"clk\" }, delay = { one = [] } }, ",
                ),
                "assignments = [{ id = \"dead\"",
                "dead outside every rewrite cone",
            ),
        ];
        for (name, text, marker, message) in cases {
            let (lowered, graph, metadata, provenance) = materialization_inputs();
            let resolved = TopologyHintCatalog::parse("apply-errors.toml", &text)
                .unwrap()
                .resolve(&TopologyHintContext::new(
                    "gate",
                    GenerateMode::Delayful,
                    &lowered,
                    &graph,
                ))
                .unwrap();
            let error = materialize_topology(
                resolved.hints()[0].require_materialization(),
                &lowered,
                &metadata,
                &provenance,
            )
            .unwrap_err();
            assert_eq!(
                error.span().path,
                PathBuf::from("apply-errors.toml"),
                "{name}"
            );
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn unknown_fields_and_syntax_have_hint_locations() {
        let error = TopologyHintCatalog::parse("fixture/hint.toml", "unknown = 1").unwrap_err();
        assert_eq!(error.span().path, PathBuf::from("fixture/hint.toml"));
        assert_eq!(error.span().line, 1);
        assert!(error.message().contains("unknown field"));
    }

    #[test]
    fn duplicate_ids_and_invalid_expressions_are_rejected() {
        let duplicate = HINT.replace(
            "{ id = \"fallback\", name = \"q_fallback\" }",
            "{ id = \"raw\", name = \"q_fallback\" }",
        );
        assert!(
            TopologyHintCatalog::parse("hint.toml", &duplicate)
                .unwrap_err()
                .message()
                .contains("duplicate signal ID")
        );
        let nested = HINT.replace(
            "operator = \"not\", operands = [\"clk\"]",
            "operator = \"not\", operands = []",
        );
        assert!(
            TopologyHintCatalog::parse("hint.toml", &nested)
                .unwrap_err()
                .message()
                .contains("wrong arity")
        );
    }

    #[test]
    fn resolution_rejects_stale_names_terms_constraints_and_recipe_coverage() {
        let (lowered, graph) = context();
        let resolve = |text: String| {
            TopologyHintCatalog::parse("hint.toml", &text)
                .unwrap()
                .resolve(&TopologyHintContext::new(
                    "gate",
                    GenerateMode::Delayful,
                    &lowered,
                    &graph,
                ))
                .unwrap_err()
                .to_string()
        };
        assert!(
            resolve(HINT.replace("module = \"gate\"", "module = \"stale\""))
                .contains("no topology hint")
        );
        assert!(
            resolve(HINT.replace(
                "target = \"q\", expression = { atom = \"d\" }",
                "target = \"q\", expression = { atom = \"clk\" }"
            ))
            .contains("does not identify")
        );
        assert!(
            resolve(HINT.replace("[\"TR\"]", "[\"MISSING\"]")).contains("not a resolved alias")
        );
        assert!(
            resolve(HINT.replace("path_order = 0", "path_order = 9"))
                .contains("no retained timing constraint")
        );
        let incomplete = HINT
            .lines()
            .filter(|line| !line.contains("clock-y-fall"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(resolve(incomplete).contains("missing path recipe"));
    }

    #[test]
    fn generated_timing_names_and_duplicate_recipes_are_rejected() {
        let (lowered, graph) = context();
        let collision = HINT.replace("name = \"q_raw\"", "name = \"d0\"");
        let error = TopologyHintCatalog::parse("hint.toml", &collision)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("reserved timing names"));
        let first_recipe = HINT
            .lines()
            .find(|line| line.contains("clock-y-rise"))
            .unwrap()
            .replace("clock-y-rise", "clock-y-rise-duplicate");
        let duplicate = HINT.replacen("\n]\n", &format!(",\n{first_recipe}\n]\n"), 1);
        let error = TopologyHintCatalog::parse("hint.toml", &duplicate)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("duplicate identical"));
    }

    #[test]
    fn semantic_errors_point_to_the_declared_multiline_item() {
        let (lowered, graph) = context();
        let context = TopologyHintContext::new("gate", GenerateMode::Delayful, &lowered, &graph);
        let cases = [
            (
                "unknown-operand",
                HINT.replace("operands = [\"clk\"]", "operands = [\"MISSING_OPERAND\"]"),
                "unknown atom",
            ),
            (
                "missing-term",
                HINT.replace("rise = [\"TR\"]", "rise = [\"MISSING_TERM\"]"),
                "not a resolved alias",
            ),
            (
                "stale-path",
                HINT.replace("path_order = 0", "path_order = 99"),
                "no retained timing constraint",
            ),
        ];
        for (marker, text, message) in cases {
            let line = text
                .lines()
                .position(|line| {
                    line.contains(marker)
                        || (marker == "unknown-operand" && line.contains("MISSING_OPERAND"))
                        || (marker == "missing-term" && line.contains("MISSING_TERM"))
                        || (marker == "stale-path" && line.contains("path_order = 99"))
                })
                .unwrap()
                + 1;
            let error = TopologyHintCatalog::parse("fixture/hint.toml", &text)
                .unwrap()
                .resolve(&context)
                .unwrap_err();
            assert_eq!(error.span().path, PathBuf::from("fixture/hint.toml"));
            assert_eq!(error.span().line, line, "{marker}");
            assert!(error.message().contains(message), "{}", error.message());
        }

        let duplicate_baseline = HINT.replace(
            "baseline_assignments = [{ id = \"state\", target = \"q\", expression = { atom = \"d\" } }, { id = \"output\", target = \"y\", expression = { atom = \"clk\" } }]",
            "baseline_assignments = [\n  { id = \"state\", target = \"q\", expression = { atom = \"d\" } },\n  { id = \"state\", target = \"y\", expression = { atom = \"clk\" } } # DUP_BASELINE\n]",
        );
        let line = line_of(&duplicate_baseline, "DUP_BASELINE");
        let error =
            TopologyHintCatalog::parse("fixture/hint.toml", &duplicate_baseline).unwrap_err();
        assert_eq!(error.span().path, PathBuf::from("fixture/hint.toml"));
        assert_eq!(error.span().line, line);
        assert!(error.message().contains("duplicate baseline assignment ID"));

        let stale_rewrite = HINT.replace(
            "{ anchor_id = \"output\", replacement = \"replacement\", fallback = \"fallback\", knownness_guard = \"known_guard\", exact_fallback_guard = \"fallback_guard\" }",
            "\n  { anchor_id = \"output\", replacement = \"MISSING_REPLACEMENT_STALE_REWRITE\", fallback = \"fallback\", knownness_guard = \"known_guard\", exact_fallback_guard = \"fallback_guard\" },",
        );
        let line = line_of(&stale_rewrite, "STALE_REWRITE");
        let error = TopologyHintCatalog::parse("fixture/hint.toml", &stale_rewrite)
            .unwrap()
            .resolve(&context)
            .unwrap_err();
        assert_eq!(error.span().path, PathBuf::from("fixture/hint.toml"));
        assert_eq!(error.span().line, line);
        assert!(error.message().contains("rewrite references missing"));
    }

    #[test]
    fn static_schema_rejections_are_specific() {
        let cases = [
            (
                "catalog unknown",
                "unknown = 1".to_string(),
                "unknown field",
            ),
            (
                "hint unknown",
                HINT.replace("module = \"gate\"", "module = \"gate\"\nunknown = 1"),
                "unknown field",
            ),
            (
                "nested unknown",
                HINT.replace(
                    "target_transition = \"rise\"",
                    "target_transition = \"rise\", nested_unknown = 1",
                ),
                "unknown field",
            ),
            (
                "mode",
                HINT.replace("generate_mode = \"delayful\"", "generate_mode = \"fast\""),
                "unknown generate_mode",
            ),
            (
                "operator",
                HINT.replace("operator = \"not\"", "operator = \"bogus\""),
                "uncontracted value operator",
            ),
            (
                "arity",
                HINT.replace(
                    "operator = \"not\", operands = [\"clk\"]",
                    "operator = \"not\", operands = []",
                ),
                "wrong arity",
            ),
            (
                "expression",
                HINT.replace(
                    "expression = { atom = \"clk\" }",
                    "expression = { atom = \"clk\", operator = \"not\" }",
                ),
                "exactly one",
            ),
            (
                "delay",
                HINT.replace(
                    "delay = { rise = [\"TR\"], fall = [\"TF\"] }",
                    "delay = { rise = [\"TR\"] }",
                ),
                "delay must be exactly",
            ),
            (
                "guard",
                HINT.replace("reason = \"knownness\"", "reason = \"bad\""),
                "unknown routing guard reason",
            ),
            (
                "transition",
                HINT.replace(
                    "target_transition = \"rise\"",
                    "target_transition = \"sideways\"",
                ),
                "unknown transition",
            ),
            (
                "ref union",
                HINT.replace(
                    "assignment = { generated = \"inv\" }",
                    "assignment = { generated = \"inv\", rewrite = \"output\" }",
                ),
                "exactly one",
            ),
        ];
        for (name, text, message) in cases {
            let error = TopologyHintCatalog::parse("schema.toml", &text).unwrap_err();
            assert_eq!(error.span().path, PathBuf::from("schema.toml"), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn additional_static_schema_rejections_have_item_locations() {
        let cases = [
            (
                "expression without atom or operator",
                HINT.replacen(
                    "expression = { atom = \"clk\" }",
                    "expression = { operands = [] }",
                    1,
                ),
                "expression = { operands = [] }",
                "expression must contain exactly one",
            ),
            (
                "expression atom with operands",
                HINT.replacen(
                    "expression = { atom = \"clk\" }",
                    "expression = { atom = \"clk\", operands = [\"clk\"] }",
                    1,
                ),
                "operands = [\"clk\"]",
                "expression must contain exactly one",
            ),
            (
                "nested expression operand",
                HINT.replace("operands = [\"clk\"]", "operands = [{ atom = \"clk\" }]"),
                "operands = [{ atom = \"clk\" }]",
                "invalid type",
            ),
            (
                "one delay mixed with rise fall",
                HINT.replace(
                    "delay = { rise = [\"TR\"], fall = [\"TF\"] }",
                    "delay = { one = [], rise = [\"TR\"], fall = [\"TF\"] }",
                ),
                "delay = { one = []",
                "delay must be exactly",
            ),
            (
                "turn-off without rise fall",
                HINT.replace(
                    "delay = { rise = [\"TR\"], fall = [\"TF\"] }",
                    "delay = { turn_off = [\"TZ\"] }",
                ),
                "delay = { turn_off",
                "delay must be exactly",
            ),
            (
                "empty assignment reference",
                HINT.replace("assignment = { generated = \"inv\" }", "assignment = {}"),
                "assignment = {}",
                "path step assignment must contain exactly one",
            ),
            (
                "empty signal id",
                HINT.replace(
                    "id = \"raw\", name = \"q_raw\"",
                    "id = \"\", name = \"q_raw\"",
                ),
                "id = \"\", name = \"q_raw\"",
                "signal must be a non-empty atom",
            ),
            (
                "whitespace signal id",
                HINT.replace(
                    "id = \"raw\", name = \"q_raw\"",
                    "id = \" \", name = \"q_raw\"",
                ),
                "id = \" \", name = \"q_raw\"",
                "signal must be a non-empty atom",
            ),
            (
                "empty signal name",
                HINT.replace(
                    "id = \"raw\", name = \"q_raw\"",
                    "id = \"raw\", name = \"\"",
                ),
                "id = \"raw\", name = \"\"",
                "signal name must be a non-empty atom",
            ),
            (
                "whitespace signal name",
                HINT.replace(
                    "id = \"raw\", name = \"q_raw\"",
                    "id = \"raw\", name = \" \"",
                ),
                "id = \"raw\", name = \" \"",
                "signal name must be a non-empty atom",
            ),
            (
                "empty assignment id",
                HINT.replace(
                    "id = \"inv\", target = \"raw\"",
                    "id = \"\", target = \"raw\"",
                ),
                "id = \"\", target = \"raw\"",
                "assignment must be a non-empty atom",
            ),
            (
                "whitespace assignment id",
                HINT.replace(
                    "id = \"inv\", target = \"raw\"",
                    "id = \" \", target = \"raw\"",
                ),
                "id = \" \", target = \"raw\"",
                "assignment must be a non-empty atom",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = TopologyHintCatalog::parse("schema-extra.toml", &text).unwrap_err();
            assert_eq!(
                error.span().path,
                PathBuf::from("schema-extra.toml"),
                "{name}"
            );
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn resolution_selects_only_the_exact_module_and_mode() {
        let (lowered, graph) = context();
        let other_module = HINT.replacen("module = \"gate\"", "module = \"other\"", 1);
        let other_mode = HINT.replacen(
            "generate_mode = \"delayful\"",
            "generate_mode = \"nodelay\"",
            1,
        );
        let catalog = TopologyHintCatalog::parse(
            "selection.toml",
            &format!("{HINT}\n{other_module}\n{other_mode}"),
        )
        .unwrap();
        let resolved = catalog
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        assert_eq!(resolved.hints().len(), 1);
        assert_eq!(resolved.hints()[0].hint().module(), "gate");
        assert_eq!(
            resolved.hints()[0].hint().generate_mode(),
            GenerateMode::Delayful
        );

        let mut stale_module = lowered.clone();
        stale_module.cell.name = "stale-module".into();
        let error = catalog
            .resolve(&TopologyHintContext::new(
                "stale-module",
                GenerateMode::Delayful,
                &stale_module,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("no topology hint for module"));
        assert!(error.message().contains("in delayful mode"));
        assert_eq!(
            error.span().path,
            PathBuf::from("<topology-hint-selection>")
        );

        let delayful_only = TopologyHintCatalog::parse("selection.toml", HINT).unwrap();
        let error = delayful_only
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Nodelay,
                &lowered,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("no topology hint for module"));
        assert!(error.message().contains("in nodelay mode"));
        assert_eq!(
            error.span().path,
            PathBuf::from("<topology-hint-selection>")
        );

        let error = delayful_only
            .resolve(&TopologyHintContext::new(
                "contradictory-module",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("context module"));
        assert!(error.message().contains("contradicts lowered cell"));
        assert_eq!(error.span().path, PathBuf::from("<topology-hint-context>"));
    }

    #[test]
    fn optional_resolution_only_selects_an_exact_noncontradictory_context() {
        let (lowered, graph) = context();
        let catalog = TopologyHintCatalog::parse("optional.toml", HINT).unwrap();

        let resolved = catalog
            .resolve_optional(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap()
            .unwrap();
        assert_eq!(resolved.hints().len(), 1);
        assert_eq!(resolved.hints()[0].hint().module(), "gate");

        let mut other_lowered = lowered.clone();
        other_lowered.cell.name = "other".into();
        assert!(
            catalog
                .resolve_optional(&TopologyHintContext::new(
                    "other",
                    GenerateMode::Delayful,
                    &other_lowered,
                    &graph,
                ))
                .unwrap()
                .is_none()
        );
        assert!(
            catalog
                .resolve_optional(&TopologyHintContext::new(
                    "gate",
                    GenerateMode::Nodelay,
                    &lowered,
                    &graph,
                ))
                .unwrap()
                .is_none()
        );

        let error = catalog
            .resolve_optional(&TopologyHintContext::new(
                "stale",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap_err();
        assert!(error.message().contains("context module stale"));
        assert!(error.message().contains("contradicts lowered cell gate"));
        assert_eq!(error.span().path, PathBuf::from("<topology-hint-context>"));
    }

    #[test]
    fn resolution_rejects_invalid_baseline_bindings_and_roles() {
        let (lowered, graph) = context();
        let cases = [
            (
                "unused stale named baseline anchor",
                HINT.replace(
                    "{ id = \"output\", target = \"y\", expression = { atom = \"clk\" } }",
                    "{ id = \"output\", target = \"STALE_NAMED\", expression = { atom = \"clk\" } }",
                ),
                "STALE_NAMED",
                "baseline assignment",
            ),
            (
                "duplicate structural named baseline anchor",
                HINT.replace(
                    "{ id = \"output\", target = \"y\", expression = { atom = \"clk\" } }",
                    "{ id = \"output\", target = \"q\", expression = { atom = \"d\" } }",
                ),
                "{ id = \"output\", target = \"q\"",
                "duplicates an existing structural anchor",
            ),
            (
                "state role target is not a register",
                HINT.replace(
                    "state = { target = \"q\", expression = { atom = \"d\" } }",
                    "state = { target = \"y\", expression = { atom = \"clk\" } }",
                ),
                "state = { target = \"y\"",
                "is not a modeled register",
            ),
            (
                "state role expression has no named baseline match",
                HINT.replace(
                    "state = { target = \"q\", expression = { atom = \"d\" } }",
                    "state = { target = \"q\", expression = { atom = \"clk\" } }",
                ),
                "state = { target = \"q\", expression = { atom = \"clk\" } }",
                "state role must map to exactly one named baseline assignment",
            ),
            (
                "output role expression has no named baseline match",
                HINT.replace(
                    "outputs = [{ target = \"y\", expression = { atom = \"clk\" } }]",
                    "outputs = [{ target = \"y\", expression = { atom = \"d\" } }]",
                ),
                "outputs = [{ target = \"y\", expression = { atom = \"d\" } }]",
                "output role must map to exactly one named baseline assignment",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "baseline.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(error.span().path, PathBuf::from("baseline.toml"), "{name}");
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }

        // Two named assignments for one role would make the role ambiguous,
        // but structural binding rejects it first and deterministically.
        let ambiguous = HINT.replace(
            "{ id = \"output\", target = \"y\", expression = { atom = \"clk\" } }",
            "{ id = \"output\", target = \"q\", expression = { atom = \"d\" } }",
        );
        let error = resolve_error(
            "baseline.toml",
            &ambiguous,
            "gate",
            GenerateMode::Delayful,
            &lowered,
            &graph,
        );
        assert!(
            error
                .message()
                .contains("duplicates an existing structural anchor")
        );

        let mut output_non_role_lowered = lowered.clone();
        output_non_role_lowered
            .cell
            .items
            .push(CellItem::Assignment(Assignment {
                target: "internal".into(),
                expr: Expr::atom("d"),
                delay: DelayTuple::One(TimingExpr::atom("0").unwrap()),
            }));
        let output_non_role = HINT
            .replace(
                "outputs = [{ target = \"y\", expression = { atom = \"clk\" } }]",
                "outputs = [{ target = \"internal\", expression = { atom = \"d\" } }]",
            )
            .replace(
                "{ id = \"output\", target = \"y\", expression = { atom = \"clk\" } }",
                "{ id = \"output\", target = \"internal\", expression = { atom = \"d\" } }",
            );
        let error = resolve_error(
            "baseline.toml",
            &output_non_role,
            "gate",
            GenerateMode::Delayful,
            &output_non_role_lowered,
            &graph,
        );
        assert_eq!(error.span().path, PathBuf::from("baseline.toml"));
        assert_eq!(
            error.span().line,
            line_of(&output_non_role, "outputs = [{ target = \"internal\"")
        );
        assert!(error.message().contains("is not an output"));
    }

    #[test]
    fn resolution_rejects_generated_name_and_reference_failures() {
        let (lowered, graph) = context();
        let cases = [
            (
                "generated name collides with baseline",
                HINT.replace("name = \"q_raw\"", "name = \"q\""),
                "name = \"q\"",
                "generated signal",
            ),
            (
                "reserved t timing name",
                HINT.replace("name = \"q_raw\"", "name = \"t0\""),
                "name = \"t0\"",
                "reserved timing names",
            ),
            (
                "reserved d timing name",
                HINT.replace("name = \"q_raw\"", "name = \"d7\""),
                "name = \"d7\"",
                "reserved timing names",
            ),
            (
                "assignment target missing generated signal",
                HINT.replace(
                    "{ id = \"inv\", target = \"raw\"",
                    "{ id = \"inv\", target = \"MISSING_SIGNAL\"",
                ),
                "target = \"MISSING_SIGNAL\"",
                "targets missing generated signal",
            ),
            (
                "assignment expression unknown atom",
                HINT.replace("operands = [\"clk\"]", "operands = [\"MISSING_ATOM\"]"),
                "MISSING_ATOM",
                "overlay expression references unknown atom",
            ),
            (
                "path step missing generated assignment",
                HINT.replace(
                    "assignment = { generated = \"inv\" }",
                    "assignment = { generated = \"MISSING_GENERATED\" }",
                ),
                "MISSING_GENERATED",
                "references missing assignment",
            ),
            (
                "path step missing baseline assignment",
                HINT.replace(
                    "assignment = { generated = \"inv\" }",
                    "assignment = { baseline_id = \"MISSING_BASELINE\" }",
                ),
                "MISSING_BASELINE",
                "references missing baseline assignment",
            ),
            (
                "path step missing rewrite",
                HINT.replace(
                    "assignment = { rewrite = \"output\" }",
                    "assignment = { rewrite = \"MISSING_REWRITE\" }",
                ),
                "MISSING_REWRITE",
                "references missing rewrite",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "references.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(
                error.span().path,
                PathBuf::from("references.toml"),
                "{name}"
            );
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn resolution_rejects_rewrite_coverage_references_and_generated_targets() {
        let (lowered, graph) = context();
        let cases = [
            (
                "missing state rewrite",
                HINT.replace(
                    "{ anchor_id = \"state\", replacement = \"state_replacement\", fallback = \"state_fallback\", knownness_guard = \"state_known_guard\", exact_fallback_guard = \"state_fallback_guard\" }, ",
                    "",
                ),
                "state = { target = \"q\"",
                "missing rewrite for baseline",
            ),
            (
                "missing output rewrite",
                HINT.replace(
                    ", { anchor_id = \"output\", replacement = \"replacement\", fallback = \"fallback\", knownness_guard = \"known_guard\", exact_fallback_guard = \"fallback_guard\" }",
                    "",
                ),
                "outputs = [{ target = \"y\"",
                "missing rewrite for baseline",
            ),
            (
                "duplicate rewrite",
                HINT.replace(
                    "rewrites = [",
                    "rewrites = [{ anchor_id = \"state\", replacement = \"state_replacement\", fallback = \"state_fallback\", knownness_guard = \"state_known_guard\", exact_fallback_guard = \"state_fallback_guard\" }, ",
                ),
                "rewrites = [",
                "duplicate rewrite for baseline",
            ),
            (
                "missing replacement",
                HINT.replace(
                    "anchor_id = \"output\", replacement = \"replacement\"",
                    "anchor_id = \"output\", replacement = \"MISSING_REPLACEMENT\"",
                ),
                "MISSING_REPLACEMENT",
                "rewrite references missing assignment",
            ),
            (
                "missing fallback",
                HINT.replace(
                    "replacement = \"replacement\", fallback = \"fallback\"",
                    "replacement = \"replacement\", fallback = \"MISSING_FALLBACK\"",
                ),
                "MISSING_FALLBACK",
                "rewrite references missing fallback assignment",
            ),
            (
                "missing knownness guard",
                HINT.replace(
                    "knownness_guard = \"known_guard\"",
                    "knownness_guard = \"MISSING_KNOWN_GUARD\"",
                ),
                "MISSING_KNOWN_GUARD",
                "rewrite references missing routing guard",
            ),
            (
                "missing exact fallback guard",
                HINT.replace(
                    "exact_fallback_guard = \"fallback_guard\"",
                    "exact_fallback_guard = \"MISSING_FALLBACK_GUARD\"",
                ),
                "MISSING_FALLBACK_GUARD",
                "rewrite references missing routing guard",
            ),
            (
                "replacement target missing signal",
                HINT.replace(
                    "{ id = \"replacement\", target = \"replacement\"",
                    "{ id = \"replacement\", target = \"MISSING_REPLACEMENT_SIGNAL\"",
                ),
                "MISSING_REPLACEMENT_SIGNAL",
                "targets missing generated signal",
            ),
            (
                "fallback target missing signal",
                HINT.replace(
                    "{ id = \"fallback\", target = \"fallback\"",
                    "{ id = \"fallback\", target = \"MISSING_FALLBACK_SIGNAL\"",
                ),
                "rewrites = [",
                "fallback assignment",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "rewrite.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(error.span().path, PathBuf::from("rewrite.toml"), "{name}");
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn resolution_rejects_rewrite_structure_and_guard_contracts() {
        let (lowered, graph) = context();
        let cases = [
            (
                "fallback expression differs",
                HINT.replace(
                    "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"clk\" }, delay = { one = [] } }",
                    "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"d\" }, delay = { one = [] } }",
                ),
                "rewrites = [",
                "zero-delay exact baseline-expression snapshot",
            ),
            (
                "fallback delay is nonzero",
                HINT.replace(
                    "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"clk\" }, delay = { one = [] } }",
                    "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"clk\" }, delay = { one = [\"TR\"] } }",
                ),
                "rewrites = [",
                "zero-delay exact baseline-expression snapshot",
            ),
            (
                "replacement is not mux",
                HINT.replace(
                    "{ id = \"replacement\", target = \"replacement\", expression = { operator = \"mux\", operands = [\"q_known\", \"q_raw\", \"q_fallback\"] }, delay = { one = [] } }",
                    "{ id = \"replacement\", target = \"replacement\", expression = { atom = \"clk\" }, delay = { one = [] } }",
                ),
                "rewrites = [",
                "replacement must be a flat mux",
            ),
            (
                "replacement mux fallback operand is wrong",
                HINT.replace(
                    "operands = [\"q_known\", \"q_raw\", \"q_fallback\"]",
                    "operands = [\"q_known\", \"q_raw\", \"q_raw\"]",
                ),
                "rewrites = [",
                "mux operand 2 must be the named fallback assignment target",
            ),
            (
                "knownness guard wrong reason",
                HINT.replace(
                    "id = \"known_guard\", assignment = { generated = \"replacement\" }, operand_index = 0, reason = \"knownness\"",
                    "id = \"known_guard\", assignment = { generated = \"replacement\" }, operand_index = 0, reason = \"exact-fallback\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
            (
                "knownness guard wrong assignment",
                HINT.replace(
                    "id = \"known_guard\", assignment = { generated = \"replacement\" }, operand_index = 0, reason = \"knownness\"",
                    "id = \"known_guard\", assignment = { generated = \"inv\" }, operand_index = 0, reason = \"knownness\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
            (
                "knownness guard wrong operand index",
                HINT.replace(
                    "id = \"known_guard\", assignment = { generated = \"replacement\" }, operand_index = 0, reason = \"knownness\"",
                    "id = \"known_guard\", assignment = { generated = \"replacement\" }, operand_index = 1, reason = \"knownness\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
            (
                "exact fallback guard wrong reason",
                HINT.replace(
                    "id = \"fallback_guard\", assignment = { generated = \"replacement\" }, operand_index = 2, reason = \"exact-fallback\"",
                    "id = \"fallback_guard\", assignment = { generated = \"replacement\" }, operand_index = 2, reason = \"knownness\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
            (
                "exact fallback guard wrong assignment",
                HINT.replace(
                    "id = \"fallback_guard\", assignment = { generated = \"replacement\" }, operand_index = 2, reason = \"exact-fallback\"",
                    "id = \"fallback_guard\", assignment = { generated = \"inv\" }, operand_index = 2, reason = \"exact-fallback\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
            (
                "exact fallback guard wrong operand index",
                HINT.replace(
                    "id = \"fallback_guard\", assignment = { generated = \"replacement\" }, operand_index = 2, reason = \"exact-fallback\"",
                    "id = \"fallback_guard\", assignment = { generated = \"replacement\" }, operand_index = 1, reason = \"exact-fallback\"",
                ),
                "rewrites = [",
                "rewrite guard does not match its required mux operand",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "rewrite-structure.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(
                error.span().path,
                PathBuf::from("rewrite-structure.toml"),
                "{name}"
            );
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn resolution_rejects_invalid_guard_edges_and_recipe_omissions() {
        let (lowered, graph) = context();
        let cases = [
            (
                "guard missing generated assignment",
                HINT.replace(
                    "routing_guards = [{",
                    "routing_guards = [{ id = \"extra_guard\", assignment = { generated = \"MISSING_GUARD_ASSIGNMENT\" }, operand_index = 0, reason = \"routing\" }, {",
                ),
                "MISSING_GUARD_ASSIGNMENT",
                "guard references missing assignment",
            ),
            (
                "guard operand index out of range",
                HINT.replace(
                    "routing_guards = [{",
                    "routing_guards = [{ id = \"extra_guard\", assignment = { generated = \"replacement\" }, operand_index = 99, reason = \"routing\" }, {",
                ),
                "operand_index = 99",
                "guard operand_index 99 is invalid",
            ),
            (
                "guard edge outside rewrite cone",
                HINT.replace(
                    "signals = [",
                    "signals = [{ id = \"outside\", name = \"outside\" }, ",
                )
                .replace(
                    "\nassignments = [",
                    "\nassignments = [{ id = \"outside_assignment\", target = \"outside\", expression = { atom = \"clk\" }, delay = { one = [] } }, ",
                )
                .replace(
                    "routing_guards = [{",
                    "routing_guards = [{ id = \"extra_guard\", assignment = { generated = \"outside_assignment\" }, operand_index = 0, reason = \"routing\" }, {",
                ),
                "id = \"extra_guard\"",
                "outside every rewrite replacement cone",
            ),
            (
                "unused guard declaration",
                HINT.replace(
                    "routing_guards = [{",
                    "routing_guards = [{ id = \"extra_guard\", assignment = { generated = \"replacement\" }, operand_index = 1, reason = \"routing\" }, {",
                ),
                "id = \"extra_guard\"",
                "unused routing guard",
            ),
            // A missing omission ID is also not the required terminal pair, so
            // exact-pair validation intentionally fails before missing-ID lookup.
            (
                "recipe omits missing guard",
                HINT.replace(
                    "omitted_routing_guards = [\"known_guard\", \"fallback_guard\"]",
                    "omitted_routing_guards = [\"known_guard\", \"MISSING_OMITTED_GUARD\"]",
                ),
                "MISSING_OMITTED_GUARD",
                "must omit exactly the rewrite knownness and exact-fallback guards",
            ),
            (
                "recipe omits incomplete guard set",
                HINT.replace(
                    "omitted_routing_guards = [\"known_guard\", \"fallback_guard\"]",
                    "omitted_routing_guards = [\"known_guard\"]",
                ),
                "omitted_routing_guards = [\"known_guard\"]",
                "must omit exactly the rewrite knownness and exact-fallback guards",
            ),
            (
                "recipe omits duplicate guard",
                HINT.replace(
                    "omitted_routing_guards = [\"known_guard\", \"fallback_guard\"]",
                    "omitted_routing_guards = [\"known_guard\", \"known_guard\", \"fallback_guard\"]",
                ),
                "known_guard\", \"known_guard",
                "omits duplicate routing guard",
            ),
            (
                "recipe omits unrelated guard",
                HINT.replace(
                    "omitted_routing_guards = [\"known_guard\", \"fallback_guard\"]",
                    "omitted_routing_guards = [\"known_guard\", \"state_fallback_guard\"]",
                ),
                "omitted_routing_guards = [\"known_guard\", \"state_fallback_guard\"]",
                "must omit exactly the rewrite knownness and exact-fallback guards",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "guards.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(error.span().path, PathBuf::from("guards.toml"), "{name}");
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }

        assert!(
            TopologyHintCatalog::parse("guards.toml", HINT)
                .unwrap()
                .resolve(&TopologyHintContext::new(
                    "gate",
                    GenerateMode::Delayful,
                    &lowered,
                    &graph,
                ))
                .is_ok()
        );
    }

    #[test]
    fn resolution_rejects_stale_constraints_and_selected_component_mismatches() {
        let (lowered, graph) = context();
        let cases = [
            (
                "stale path order",
                HINT.replace("path_order = 0", "path_order = 9"),
                "path_order = 9",
                "no retained timing constraint at path_order 9",
            ),
            (
                "stale control order",
                HINT.replace("control_order = 0", "control_order = 9"),
                "control_order = 9",
                "has no control_order 9",
            ),
            (
                "stale control signal",
                HINT.replace("control = \"clk\"", "control = \"d\""),
                "control = \"d\"",
                "stale retained constraint key",
            ),
            (
                "stale target signal",
                HINT.replace(
                    "control = \"clk\", target = \"y\"",
                    "control = \"clk\", target = \"q\"",
                ),
                "control = \"clk\", target = \"q\"",
                "stale retained constraint key",
            ),
            (
                "unsupported turn off component",
                HINT.replace(
                    "target_transition = \"rise\"",
                    "target_transition = \"turn-off\"",
                ),
                "target_transition = \"turn-off\"",
                "selects unsupported TurnOff component of a 2-entry delay tuple",
            ),
            (
                "missing expected timing alias",
                HINT.replace(
                    "expected_terms = [\"TR\"]",
                    "expected_terms = [\"MISSING_TERM\"]",
                ),
                "MISSING_TERM",
                "is not a resolved alias or specparam",
            ),
            (
                "rise recipe uses fall component",
                HINT.replace("expected_terms = [\"TR\"]", "expected_terms = [\"TF\"]"),
                "clock-y-rise",
                "expected terms do not exactly match its selected retained tuple component",
            ),
            (
                "fall recipe uses rise component",
                HINT.replace("expected_terms = [\"TF\"]", "expected_terms = [\"TR\"]"),
                "clock-y-fall",
                "expected terms do not exactly match its selected retained tuple component",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "constraints.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(
                error.span().path,
                PathBuf::from("constraints.toml"),
                "{name}"
            );
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }

        let resolved = TopologyHintCatalog::parse("constraints.toml", HINT)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        assert_eq!(
            graph.constraints()[0].controls()[0].source().transition(),
            None
        );
        let recipes = resolved.hints()[0].recipes();
        assert_eq!(recipes.len(), 2);
        assert_eq!(recipes[0].expected_terms.terms(), &["TR"]);
        assert_eq!(recipes[1].expected_terms.terms(), &["TF"]);
    }

    #[test]
    fn resolution_validates_recipe_coverage_variants_and_walks() {
        let (lowered, graph) = context();
        let cases = [
            (
                "empty recipe steps",
                HINT.replace(
                    "steps = [{ assignment = { generated = \"inv\" }, operand_index = 0, transition = \"rise\" }, { assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"rise\" }, { assignment = { rewrite = \"output\" }, operand_index = 0, transition = \"rise\" }]",
                    "steps = []",
                ),
                "steps = []",
                "must contain at least one typed dependency step",
            ),
            (
                "walk operand index out of range",
                HINT.replace(
                    "assignment = { generated = \"inv\" }, operand_index = 0, transition = \"rise\"",
                    "assignment = { generated = \"inv\" }, operand_index = 9, transition = \"rise\"",
                ),
                "operand_index = 9",
                "operand_index 9 is invalid",
            ),
            (
                "discontinuous predecessor and replacement edge",
                HINT.replace(
                    "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"rise\"",
                    "assignment = { generated = \"replacement\" }, operand_index = 0, transition = \"rise\"",
                ),
                "generated = \"replacement\" }, operand_index = 0, transition = \"rise\"",
                "has discontinuous dependency walk",
            ),
            (
                "walk ends at the wrong target",
                HINT.replace(
                    "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"rise\"",
                    "assignment = { generated = \"state_replacement\" }, operand_index = 1, transition = \"rise\"",
                )
                .replace(
                    "assignment = { rewrite = \"output\" }, operand_index = 0, transition = \"rise\"",
                    "assignment = { rewrite = \"state\" }, operand_index = 0, transition = \"rise\"",
                ),
                "rewrite = \"state\"",
                "ends at",
            ),
        ];
        for (name, text, marker, message) in cases {
            let error = resolve_error(
                "walk.toml",
                &text,
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            );
            assert_eq!(error.span().path, PathBuf::from("walk.toml"), "{name}");
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }

        let incomplete = HINT
            .lines()
            .filter(|line| !line.contains("clock-y-fall"))
            .collect::<Vec<_>>()
            .join("\n");
        let error = resolve_error(
            "coverage.toml",
            &incomplete,
            "gate",
            GenerateMode::Delayful,
            &lowered,
            &graph,
        );
        assert_eq!(error.span().path, PathBuf::from("coverage.toml"));
        assert!(error.message().contains("missing path recipe"));

        let duplicate_recipe = HINT
            .lines()
            .find(|line| line.contains("clock-y-rise"))
            .unwrap()
            .replace("clock-y-rise", "clock-y-rise-duplicate");
        let duplicate = HINT.replacen("\n]\n", &format!(",\n{duplicate_recipe}\n]\n"), 1);
        let error = resolve_error(
            "coverage.toml",
            &duplicate,
            "gate",
            GenerateMode::Delayful,
            &lowered,
            &graph,
        );
        assert_eq!(
            error.span().line,
            line_of(&duplicate, "clock-y-rise-duplicate")
        );
        assert!(error.message().contains("duplicate identical"));

        let alternate_recipe = HINT
            .lines()
            .find(|line| line.contains("clock-y-rise"))
            .unwrap()
            .replace("clock-y-rise", "clock-y-rise-alternate")
            .replacen(
                "operand_index = 0, transition = \"rise\"",
                "operand_index = 0, transition = \"fall\"",
                1,
            )
            .replacen(
                "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"rise\"",
                "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"fall\"",
                1,
            )
            .replacen(
                "assignment = { rewrite = \"output\" }, operand_index = 0, transition = \"rise\"",
                "assignment = { rewrite = \"output\" }, operand_index = 0, transition = \"fall\"",
                1,
            );
        let variants = HINT.replacen("\n]\n", &format!(",\n{alternate_recipe}\n]\n"), 1);
        let resolved = TopologyHintCatalog::parse("coverage.toml", &variants)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                "gate",
                GenerateMode::Delayful,
                &lowered,
                &graph,
            ))
            .unwrap();
        let recipes = resolved.hints()[0].recipes();
        assert_eq!(recipes.len(), 3);
        assert_eq!(
            recipes[2].id,
            HintPathRecipeId("clock-y-rise-alternate".into())
        );
        assert_eq!(recipes[2].steps[0].transition, Transition::Fall);

        let mut uncovered_graph = graph.clone();
        let span = Span::new("gate.sv", 2, 1);
        uncovered_graph
            .add_constraint(
                TimingConstraintSource::new(
                    1,
                    vec![TimingControlSource::new("clk", None, span.clone()).unwrap()],
                    "y",
                    DelayTuple::Two {
                        rise: TimingExpr::atom("TR").unwrap(),
                        fall: TimingExpr::atom("TF").unwrap(),
                    },
                    span,
                )
                .unwrap(),
            )
            .unwrap();
        let error = resolve_error(
            "coverage.toml",
            HINT,
            "gate",
            GenerateMode::Delayful,
            &lowered,
            &uncovered_graph,
        );
        assert!(
            error
                .message()
                .contains("missing path recipe for retained path 1")
        );

        let mut terminal_lowered = lowered.clone();
        terminal_lowered.cell.items[1] = CellItem::Assignment(Assignment {
            target: "y".into(),
            expr: Expr::atom("q_replacement"),
            delay: DelayTuple::One(TimingExpr::atom("0").unwrap()),
        });
        let no_virtual_terminal = HINT
            .replace(
                "outputs = [{ target = \"y\", expression = { atom = \"clk\" } }]",
                "outputs = [{ target = \"y\", expression = { atom = \"q_replacement\" } }]",
            )
            .replace(
                "{ id = \"output\", target = \"y\", expression = { atom = \"clk\" } }",
                "{ id = \"output\", target = \"y\", expression = { atom = \"q_replacement\" } }",
            )
            .replace(
                "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"clk\" }, delay = { one = [] } }",
                "{ id = \"fallback\", target = \"fallback\", expression = { atom = \"q_replacement\" }, delay = { one = [] } }",
            )
            .replace(
                "assignment = { rewrite = \"output\" }, operand_index = 0, transition = \"rise\"",
                "assignment = { baseline_id = \"output\" }, operand_index = 0, transition = \"rise\"",
            );
        let error = resolve_error(
            "walk.toml",
            &no_virtual_terminal,
            "gate",
            GenerateMode::Delayful,
            &terminal_lowered,
            &graph,
        );
        assert_eq!(
            error.span().line,
            line_of(&no_virtual_terminal, "baseline_id = \"output\"")
        );
        assert!(
            error
                .message()
                .contains("must terminate at a virtual rewrite step")
        );
    }

    #[test]
    fn catalog_and_generated_id_duplicates_are_static_errors() {
        let nested_unknown = HINT.replace(
            "target_transition = \"rise\"",
            "target_transition = \"rise\", nested_unknown = 1",
        );
        let error = TopologyHintCatalog::parse("nested.toml", &nested_unknown).unwrap_err();
        assert_eq!(error.span().path, PathBuf::from("nested.toml"));
        assert_eq!(
            error.span().line,
            line_of(&nested_unknown, "nested_unknown"),
            "nested unknown-field error should identify its recipe item"
        );
        assert!(error.message().contains("unknown field"));

        let second = HINT.replacen("module = \"gate\"", "module = \"other\"", 1);
        let duplicate = format!("{HINT}\n{HINT}");
        assert!(
            TopologyHintCatalog::parse("catalog.toml", &duplicate)
                .unwrap_err()
                .message()
                .contains("duplicate hint")
        );
        let other_mode = format!(
            "{HINT}\n{}",
            HINT.replacen(
                "generate_mode = \"delayful\"",
                "generate_mode = \"nodelay\"",
                1
            )
        );
        assert!(TopologyHintCatalog::parse("catalog.toml", &other_mode).is_ok());
        assert!(TopologyHintCatalog::parse("catalog.toml", &format!("{HINT}\n{second}")).is_ok());
        for (text, message, marker) in [
            (
                HINT.replace(
                    "{ id = \"fallback\", target = \"fallback\"",
                    "{ id = \"inv\", target = \"fallback\"",
                ),
                "duplicate assignment ID",
                "target = \"fallback\"",
            ),
            (
                HINT.replace("id = \"fallback_guard\"", "id = \"known_guard\""),
                "duplicate routing guard ID",
                "id = \"known_guard\"",
            ),
            (
                HINT.replace("id = \"clock-y-fall\"", "id = \"clock-y-rise\""),
                "duplicate path recipe ID",
                "target_transition = \"fall\"",
            ),
        ] {
            let error = TopologyHintCatalog::parse("catalog.toml", &text).unwrap_err();
            assert_eq!(error.span().path, PathBuf::from("catalog.toml"));
            assert_eq!(error.span().line, line_of(&text, marker));
            assert!(error.message().contains(message), "{}", error.message());
        }
    }

    #[test]
    fn resolution_rejects_transition_inconsistent_unate_recipe_step() {
        let (lowered, graph) = context();
        let text = HINT.replacen(
            "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"rise\"",
            "assignment = { generated = \"replacement\" }, operand_index = 1, transition = \"fall\"",
            1,
        );
        let error = resolve_error(
            "unate.toml",
            &text,
            "gate",
            GenerateMode::Delayful,
            &lowered,
            &graph,
        );
        assert_eq!(error.span().path, PathBuf::from("unate.toml"));
        assert_eq!(
            error.span().line,
            line_of(&text, "generated = \"replacement\" }, operand_index = 1")
        );
        assert!(
            error
                .message()
                .contains("transition-inconsistent unate edge")
        );
    }

    #[test]
    fn unate_transition_validation_is_conservative_for_conditional_and_turn_off_edges() {
        let conditional = TopologyValueExpr::Operation {
            operator: ValueOperator::BufIf0,
            operands: vec!["d".into(), "enable".into()],
        };
        assert_eq!(
            unate_output_transition(&conditional, 0, Transition::Rise),
            Some(Transition::Rise)
        );
        assert_eq!(
            unate_output_transition(&conditional, 1, Transition::Rise),
            None
        );
        let inverted = TopologyValueExpr::Operation {
            operator: ValueOperator::Not,
            operands: vec!["d".into()],
        };
        assert_eq!(
            unate_output_transition(&inverted, 0, Transition::TurnOff),
            None
        );
    }

    fn line_of(text: &str, marker: &str) -> usize {
        text.lines().position(|line| line.contains(marker)).unwrap() + 1
    }

    fn resolve_error(
        path: &str,
        text: &str,
        module: &str,
        generate_mode: GenerateMode,
        lowered: &LoweredModule,
        graph: &TimingGraph,
    ) -> TopologyHintError {
        TopologyHintCatalog::parse(path, text)
            .unwrap()
            .resolve(&TopologyHintContext::new(
                module,
                generate_mode,
                lowered,
                graph,
            ))
            .unwrap_err()
    }
}
