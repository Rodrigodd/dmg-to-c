//! Independent verification of a resolved topology hint against its actual
//! materialized cell and timing graph.

use std::collections::BTreeSet;
use std::fmt;

use crate::diagnostic::Span;
use crate::ir::{CellItem, DelayTuple, Expr, TimingExpr};
use crate::timing_graph::{
    AssignmentDelayOrigin, DependencyKind, TimingConstraint, TimingConstraintId, TimingControlId,
    TimingGraph, TimingNodeId, TimingSense, Transition, TransitionEffect, propagate_transition,
};
use crate::timing_terms::AdditiveDelay;
use crate::topology_apply::AppliedTopologyTransform;
use crate::topology_hint::{
    ResolvedPathRecipe, ResolvedPathStepKind, ResolvedRecipeIngress, ResolvedTopologyHint,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyVerificationError {
    span: Span,
    message: String,
}

impl TopologyVerificationError {
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

impl fmt::Display for TopologyVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: topology verification: {}",
            self.span.path.display(),
            self.span.line,
            self.span.column,
            self.message
        )
    }
}
impl std::error::Error for TopologyVerificationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTopologyStep {
    kind: ResolvedPathStepKind,
    assignment_order: usize,
    assignment_node: TimingNodeId,
    operand_index: usize,
    transition: Transition,
    dependency_order: usize,
}
impl VerifiedTopologyStep {
    pub fn kind(&self) -> &ResolvedPathStepKind {
        &self.kind
    }
    pub const fn assignment_order(&self) -> usize {
        self.assignment_order
    }
    pub const fn assignment_node(&self) -> TimingNodeId {
        self.assignment_node
    }
    pub const fn operand_index(&self) -> usize {
        self.operand_index
    }
    pub const fn transition(&self) -> Transition {
        self.transition
    }
    pub const fn dependency_order(&self) -> usize {
        self.dependency_order
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTopologyPath {
    recipe: String,
    constraint: TimingConstraintId,
    control: TimingControlId,
    target_transition: Transition,
    steps: Vec<VerifiedTopologyStep>,
    terms: Vec<TimingExpr>,
}
impl VerifiedTopologyPath {
    pub fn recipe(&self) -> &str {
        &self.recipe
    }
    pub const fn constraint(&self) -> TimingConstraintId {
        self.constraint
    }
    pub const fn control(&self) -> TimingControlId {
        self.control
    }
    pub const fn target_transition(&self) -> Transition {
        self.target_transition
    }
    pub fn steps(&self) -> &[VerifiedTopologyStep] {
        &self.steps
    }
    pub fn terms(&self) -> &[TimingExpr] {
        &self.terms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualTopologyVerification {
    paths: Vec<VerifiedTopologyPath>,
}
impl ActualTopologyVerification {
    pub fn paths(&self) -> &[VerifiedTopologyPath] {
        &self.paths
    }
}

pub fn verify_materialized_topology(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
    graph: &TimingGraph,
) -> Result<ActualTopologyVerification, TopologyVerificationError> {
    verify_facts(hint, applied)?;
    let mut paths = Vec::with_capacity(hint.recipes().len());
    for recipe in hint.recipes() {
        paths.push(verify_recipe(hint, applied, graph, recipe)?);
    }
    verify_resolved_assignment_delays(hint, applied)?;
    Ok(ActualTopologyVerification { paths })
}

fn verify_facts(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
) -> Result<(), TopologyVerificationError> {
    if applied.facts.assignments.len() != hint.assignments().len() {
        return Err(TopologyVerificationError::new(
            hint.assignments()
                .first()
                .map(|assignment| assignment.span().clone())
                .unwrap_or_else(|| Span::new("<topology-verify>", 1, 1)),
            "materialized generated assignment facts do not cover the resolved hint",
        ));
    }
    for assignment in hint.assignments() {
        let fact = applied
            .facts
            .assignments
            .get(assignment.id())
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    assignment.span().clone(),
                    "missing materialized assignment fact",
                )
            })?;
        let actual =
            assignment_at(&applied.lowered.cell.items, fact.assignment_order).ok_or_else(|| {
                TopologyVerificationError::new(
                    assignment.span().clone(),
                    "materialized assignment order is absent",
                )
            })?;
        if actual != &fact.assignment || actual.target != assignment.target_name() {
            return Err(TopologyVerificationError::new(
                assignment.span().clone(),
                "materialized assignment fact does not match the actual cell assignment",
            ));
        }
        if actual.expr != assignment.expression().to_expr() {
            return Err(TopologyVerificationError::new(
                actual_span(applied, fact.assignment_order, assignment.span()),
                "actual assignment expression does not match resolved topology shape",
            ));
        }
        let provenance = applied
            .provenance
            .get(fact.assignment_order)
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    assignment.span().clone(),
                    "materialized provenance is absent",
                )
            })?;
        if !provenance.origin().is_topology_generated()
            || provenance.delay_origin() != AssignmentDelayOrigin::TopologyPlacement
        {
            return Err(TopologyVerificationError::new(
                provenance.span().clone(),
                "materialized topology assignment has incompatible provenance",
            ));
        }
    }
    let original_count = applied
        .lowered
        .cell
        .items
        .iter()
        .filter(|item| matches!(item, CellItem::Assignment(_)))
        .count()
        .saturating_sub(hint.assignments().len());
    if applied.facts.original_assignment_orders.len() != original_count {
        return Err(TopologyVerificationError::new(
            Span::new("<topology-verify>", 1, 1),
            "materialized original assignment-order map is incomplete",
        ));
    }
    let expected_original_orders = (0..original_count).collect::<BTreeSet<_>>();
    let actual_original_orders = applied
        .facts
        .original_assignment_orders
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    if actual_original_orders != expected_original_orders {
        return Err(TopologyVerificationError::new(
            Span::new("<topology-verify>", 1, 1),
            "materialized original assignment-order map keys are not exact",
        ));
    }
    let transformed_orders = applied
        .facts
        .original_assignment_orders
        .values()
        .copied()
        .collect::<BTreeSet<_>>();
    if transformed_orders.len() != original_count
        || transformed_orders
            .iter()
            .any(|order| *order >= applied.provenance.len())
        || transformed_orders
            .iter()
            .any(|order| applied.provenance[*order].origin().is_topology_generated())
    {
        return Err(TopologyVerificationError::new(
            Span::new("<topology-verify>", 1, 1),
            "materialized original assignment-order map is stale or non-injective",
        ));
    }
    if applied.facts.rewrites.len() != hint.rewrites().len() {
        return Err(TopologyVerificationError::new(
            Span::new("<topology-verify>", 1, 1),
            "materialized rewrite facts do not cover the resolved hint",
        ));
    }
    for rewrite in hint.rewrites() {
        let fact = applied
            .facts
            .rewrites
            .get(&rewrite.baseline)
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    rewrite.span.clone(),
                    "materialized rewrite fact is absent",
                )
            })?;
        let actual =
            assignment_at(&applied.lowered.cell.items, fact.assignment_order).ok_or_else(|| {
                TopologyVerificationError::new(
                    rewrite.span.clone(),
                    "materialized rewrite assignment is absent",
                )
            })?;
        if actual != &fact.after
            || actual.expr
                != Expr::atom(
                    hint.assignment(&rewrite.replacement)
                        .ok_or_else(|| {
                            TopologyVerificationError::new(
                                rewrite.span.clone(),
                                "rewrite replacement is absent",
                            )
                        })?
                        .target_name(),
                )
        {
            return Err(TopologyVerificationError::new(
                actual_span(applied, fact.assignment_order, &rewrite.span),
                "materialized rewrite fact does not match actual replacement",
            ));
        }
    }
    Ok(())
}

fn verify_recipe(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
    graph: &TimingGraph,
    recipe: &ResolvedPathRecipe,
) -> Result<VerifiedTopologyPath, TopologyVerificationError> {
    let constraint = graph
        .constraints()
        .iter()
        .find(|constraint| {
            constraint.path_order() == recipe.path_order && constraint.target() == recipe.target
        })
        .ok_or_else(|| {
            TopologyVerificationError::new(
                recipe.span.clone(),
                "retained timing constraint is absent",
            )
        })?;
    let control = constraint
        .controls()
        .get(recipe.control_order)
        .ok_or_else(|| {
            TopologyVerificationError::new(recipe.span.clone(), "retained timing control is absent")
        })?;
    let mut input = ingress_signal(hint, applied, graph, recipe, constraint)?;
    let mut prior_transition = None;
    let mut verified = Vec::new();
    let mut reconstructed = Vec::new();
    for step in &recipe.steps {
        let (kind, order, actual) = actual_step(hint, applied, &step.kind, &recipe.span)?;
        let operand = expr_operand(&actual.expr, step.operand_index).ok_or_else(|| {
            TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "actual step operand is absent",
            )
        })?;
        if operand != input {
            return Err(TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "actual materialized recipe walk is discontinuous",
            ));
        }
        let source = graph.signal_id(&input).ok_or_else(|| {
            TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "walk source signal is absent from graph",
            )
        })?;
        let target = graph.assignment_id(order).ok_or_else(|| {
            TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "walk assignment node is absent from graph",
            )
        })?;
        let operand_edges = graph
            .dependencies()
            .iter()
            .enumerate()
            .filter(|(_, dependency)| {
                dependency.source() == source
                    && dependency.target() == target
                    && dependency.edge().kind() == DependencyKind::Operand
                    && dependency.edge().operand_index() == Some(step.operand_index)
            })
            .collect::<Vec<_>>();
        let [(dependency_order, edge)] = operand_edges.as_slice() else {
            return Err(TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "actual operand dependency edge is missing or duplicated",
            ));
        };
        let output = graph.signal_id(&actual.target).ok_or_else(|| {
            TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "assignment target signal is absent from graph",
            )
        })?;
        let result_edges = graph
            .dependencies()
            .iter()
            .filter(|dependency| {
                dependency.source() == target
                    && dependency.target() == output
                    && matches!(
                        dependency.edge().kind(),
                        DependencyKind::Drive
                            | DependencyKind::StateBoundary
                            | DependencyKind::ResolvedNetBoundary
                    )
            })
            .collect::<Vec<_>>();
        if result_edges.len() != 1 {
            return Err(TopologyVerificationError::new(
                actual_span(applied, order, &recipe.span),
                "actual assignment-result boundary edge is missing or duplicated",
            ));
        }
        if let Some(previous) = prior_transition {
            verify_actual_transition(
                edge.edge().sense(),
                previous,
                step.transition,
                edge.edge().span(),
            )?;
        }
        reconstructed.extend(selected_terms(
            &actual.delay,
            step.transition,
            &recipe.span,
        )?);
        verified.push(VerifiedTopologyStep {
            kind,
            assignment_order: order,
            assignment_node: target,
            operand_index: step.operand_index,
            transition: step.transition,
            dependency_order: *dependency_order,
        });
        input = actual.target.clone();
        prior_transition = Some(step.transition);
    }
    verify_recipe_terminal_and_guards(hint, applied, recipe, &verified, graph)?;
    let component = delay_component(constraint, recipe.transition, &recipe.span)?;
    let source_terms = nonzero_terms(component, &recipe.span)?;
    if reconstructed != source_terms {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "actual materialized delay terms do not reconstruct the source timing component",
        ));
    }
    let mut expected_terms = Vec::new();
    for term in recipe.expected_terms.terms() {
        let expression = hint.alias_terms().get(term).cloned().ok_or_else(|| {
            TopologyVerificationError::new(
                recipe.span.clone(),
                "resolved expected timing term is absent",
            )
        })?;
        let flattened = AdditiveDelay::from_timing_expr(expression).map_err(|error| {
            TopologyVerificationError::new(recipe.span.clone(), error.to_string())
        })?;
        expected_terms.extend(nonzero_terms(&flattened, &recipe.span)?);
    }
    if expected_terms != source_terms {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "resolved expected terms drift from source timing component",
        ));
    }
    Ok(VerifiedTopologyPath {
        recipe: recipe.id.as_str().into(),
        constraint: constraint.id(),
        control: control.id(),
        target_transition: recipe.transition,
        steps: verified,
        terms: reconstructed,
    })
}

fn verify_actual_transition(
    sense: TimingSense,
    input: Transition,
    declared: Transition,
    span: &Span,
) -> Result<(), TopologyVerificationError> {
    match propagate_transition(sense, input) {
        TransitionEffect::Exact(expected) if expected != declared => {
            Err(TopologyVerificationError::new(
                span.clone(),
                "actual unate dependency edge contradicts the declared local transition",
            ))
        }
        TransitionEffect::Exact(_) | TransitionEffect::Indeterminate => Ok(()),
    }
}

fn verify_resolved_assignment_delays(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
) -> Result<(), TopologyVerificationError> {
    for resolved in hint.assignments() {
        let fact = applied
            .facts
            .assignments
            .get(resolved.id())
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    resolved.span().clone(),
                    "resolved assignment fact is absent",
                )
            })?;
        let actual =
            assignment_at(&applied.lowered.cell.items, fact.assignment_order).ok_or_else(|| {
                TopologyVerificationError::new(
                    resolved.span().clone(),
                    "resolved assignment is absent",
                )
            })?;
        if actual.delay != *resolved.delay() {
            return Err(TopologyVerificationError::new(
                actual_span(applied, fact.assignment_order, resolved.span()),
                "actual assignment delay differs from resolved topology assignment",
            ));
        }
    }
    Ok(())
}

fn ingress_signal(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
    graph: &TimingGraph,
    recipe: &ResolvedPathRecipe,
    constraint: &TimingConstraint,
) -> Result<String, TopologyVerificationError> {
    let control = constraint
        .controls()
        .get(recipe.control_order)
        .ok_or_else(|| {
            TopologyVerificationError::new(recipe.span.clone(), "ingress control is absent")
        })?
        .source()
        .signal();
    match &recipe.ingress {
        ResolvedRecipeIngress::DirectControl => {
            if graph.signal_id(control).is_none() {
                return Err(TopologyVerificationError::new(
                    recipe.span.clone(),
                    "direct ingress control signal is absent from graph",
                ));
            }
            Ok(control.into())
        }
        ResolvedRecipeIngress::BaselineBuffer(id) => {
            let baseline = hint.baseline_assignment(id).ok_or_else(|| {
                TopologyVerificationError::new(
                    recipe.span.clone(),
                    "ingress baseline anchor is absent",
                )
            })?;
            let order = applied
                .facts
                .original_assignment_orders
                .get(&baseline.assignment_order())
                .copied()
                .ok_or_else(|| {
                    TopologyVerificationError::new(
                        recipe.span.clone(),
                        "ingress baseline assignment order is absent",
                    )
                })?;
            let actual = assignment_at(&applied.lowered.cell.items, order).ok_or_else(|| {
                TopologyVerificationError::new(
                    recipe.span.clone(),
                    "ingress baseline assignment is absent",
                )
            })?;
            if actual.delay
                != DelayTuple::One(TimingExpr::atom("0").map_err(|error| {
                    TopologyVerificationError::new(recipe.span.clone(), error.to_string())
                })?)
                || actual.expr != Expr::atom(control)
                || actual.target != baseline.anchor().target()
            {
                return Err(TopologyVerificationError::new(
                    actual_span(applied, order, &recipe.span),
                    "typed baseline ingress is not an actual direct zero-delay control buffer",
                ));
            }
            let provenance = applied.provenance.get(order).ok_or_else(|| {
                TopologyVerificationError::new(recipe.span.clone(), "ingress provenance is absent")
            })?;
            if provenance.origin().is_topology_generated() {
                return Err(TopologyVerificationError::new(
                    provenance.span().clone(),
                    "baseline ingress unexpectedly has topology provenance",
                ));
            }
            let control_node = graph.signal_id(control).ok_or_else(|| {
                TopologyVerificationError::new(
                    recipe.span.clone(),
                    "ingress control signal is absent from graph",
                )
            })?;
            let assignment_node = graph.assignment_id(order).ok_or_else(|| {
                TopologyVerificationError::new(
                    provenance.span().clone(),
                    "ingress assignment node is absent",
                )
            })?;
            let target_node = graph.signal_id(&actual.target).ok_or_else(|| {
                TopologyVerificationError::new(
                    provenance.span().clone(),
                    "ingress output signal is absent",
                )
            })?;
            let input_edges = graph
                .dependencies()
                .iter()
                .filter(|edge| {
                    edge.source() == control_node
                        && edge.target() == assignment_node
                        && edge.edge().kind() == DependencyKind::Operand
                        && edge.edge().operand_index() == Some(0)
                })
                .count();
            let result_edges = graph
                .dependencies()
                .iter()
                .filter(|edge| {
                    edge.source() == assignment_node
                        && edge.target() == target_node
                        && matches!(
                            edge.edge().kind(),
                            DependencyKind::Drive
                                | DependencyKind::StateBoundary
                                | DependencyKind::ResolvedNetBoundary
                        )
                })
                .count();
            if input_edges != 1 || result_edges != 1 {
                return Err(TopologyVerificationError::new(
                    provenance.span().clone(),
                    "typed baseline ingress graph edges are missing or duplicated",
                ));
            }
            Ok(actual.target.clone())
        }
    }
}

fn actual_step<'a>(
    hint: &ResolvedTopologyHint,
    applied: &'a AppliedTopologyTransform,
    kind: &ResolvedPathStepKind,
    span: &Span,
) -> Result<(ResolvedPathStepKind, usize, &'a crate::ir::Assignment), TopologyVerificationError> {
    match kind {
        ResolvedPathStepKind::Generated(id) => {
            let fact = applied.facts.assignments.get(id).ok_or_else(|| {
                TopologyVerificationError::new(span.clone(), "generated step fact is absent")
            })?;
            let actual = assignment_at(&applied.lowered.cell.items, fact.assignment_order)
                .ok_or_else(|| {
                    TopologyVerificationError::new(
                        span.clone(),
                        "generated step assignment is absent",
                    )
                })?;
            Ok((kind.clone(), fact.assignment_order, actual))
        }
        ResolvedPathStepKind::Baseline(id) => {
            let baseline = hint.baseline_assignment(id).ok_or_else(|| {
                TopologyVerificationError::new(span.clone(), "baseline step anchor is absent")
            })?;
            let order = applied
                .facts
                .original_assignment_orders
                .get(&baseline.assignment_order())
                .copied()
                .ok_or_else(|| {
                    TopologyVerificationError::new(span.clone(), "baseline step order is absent")
                })?;
            let actual = assignment_at(&applied.lowered.cell.items, order).ok_or_else(|| {
                TopologyVerificationError::new(span.clone(), "baseline step assignment is absent")
            })?;
            Ok((kind.clone(), order, actual))
        }
        ResolvedPathStepKind::Rewrite(id) => {
            let rewrite = applied.facts.rewrites.get(id).ok_or_else(|| {
                TopologyVerificationError::new(span.clone(), "rewrite step fact is absent")
            })?;
            let actual = assignment_at(&applied.lowered.cell.items, rewrite.assignment_order)
                .ok_or_else(|| {
                    TopologyVerificationError::new(
                        span.clone(),
                        "rewrite step assignment is absent",
                    )
                })?;
            Ok((kind.clone(), rewrite.assignment_order, actual))
        }
    }
}

fn verify_recipe_terminal_and_guards(
    hint: &ResolvedTopologyHint,
    applied: &AppliedTopologyTransform,
    recipe: &ResolvedPathRecipe,
    steps: &[VerifiedTopologyStep],
    graph: &TimingGraph,
) -> Result<(), TopologyVerificationError> {
    let Some(ResolvedPathStepKind::Rewrite(baseline)) = recipe.steps.last().map(|step| &step.kind)
    else {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "recipe does not terminate at rewrite",
        ));
    };
    let rewrite = hint.rewrite(baseline).ok_or_else(|| {
        TopologyVerificationError::new(recipe.span.clone(), "resolved rewrite is absent")
    })?;
    let terminal = steps.last().ok_or_else(|| {
        TopologyVerificationError::new(recipe.span.clone(), "recipe has no verified terminal")
    })?;
    if terminal.operand_index != 0 {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "virtual rewrite must consume replacement at operand zero",
        ));
    }
    let replacement = applied
        .facts
        .assignments
        .get(&rewrite.replacement)
        .ok_or_else(|| {
            TopologyVerificationError::new(recipe.span.clone(), "replacement fact is absent")
        })?;
    let expected = BTreeSet::from([
        rewrite.knownness_guard.clone(),
        rewrite.exact_fallback_guard.clone(),
    ]);
    if recipe
        .omitted_guards
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "recipe does not omit exactly declared rewrite guards",
        ));
    }
    let penultimate = steps.iter().rev().nth(1).ok_or_else(|| {
        TopologyVerificationError::new(
            recipe.span.clone(),
            "recipe lacks replacement before rewrite",
        )
    })?;
    if !matches!(&penultimate.kind, ResolvedPathStepKind::Generated(id) if id == &rewrite.replacement)
    {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "recipe does not enter virtual rewrite through declared replacement",
        ));
    }
    for guard_id in &recipe.omitted_guards {
        let guard = hint.guard(guard_id).ok_or_else(|| {
            TopologyVerificationError::new(recipe.span.clone(), "omitted guard is absent")
        })?;
        if guard.assignment != rewrite.replacement {
            return Err(TopologyVerificationError::new(
                recipe.span.clone(),
                "declared guard does not bind the rewrite replacement",
            ));
        }
        let assignment = applied
            .facts
            .assignments
            .get(&guard.assignment)
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    recipe.span.clone(),
                    "guard assignment fact is absent",
                )
            })?;
        let node = graph
            .assignment_id(assignment.assignment_order)
            .ok_or_else(|| {
                TopologyVerificationError::new(
                    recipe.span.clone(),
                    "guard assignment node is absent",
                )
            })?;
        let actual = assignment_at(&applied.lowered.cell.items, assignment.assignment_order)
            .ok_or_else(|| {
                TopologyVerificationError::new(recipe.span.clone(), "guard assignment is absent")
            })?;
        let source_name = expr_operand(&actual.expr, guard.operand_index).ok_or_else(|| {
            TopologyVerificationError::new(
                actual_span(applied, assignment.assignment_order, &recipe.span),
                "guard operand is absent from actual replacement",
            )
        })?;
        let source = graph.signal_id(&source_name).ok_or_else(|| {
            TopologyVerificationError::new(recipe.span.clone(), "guard source signal is absent")
        })?;
        let edges = graph
            .dependencies()
            .iter()
            .filter(|dependency| {
                dependency.source() == source
                    && dependency.target() == node
                    && dependency.edge().kind() == DependencyKind::Operand
                    && dependency.edge().operand_index() == Some(guard.operand_index)
            })
            .count();
        if edges != 1 {
            return Err(TopologyVerificationError::new(
                actual_span(applied, assignment.assignment_order, &recipe.span),
                "declared guard edge is missing or duplicated in actual graph",
            ));
        }
    }
    if replacement.assignment_order == terminal.assignment_order {
        return Err(TopologyVerificationError::new(
            recipe.span.clone(),
            "rewrite terminal is not distinct from replacement",
        ));
    }
    Ok(())
}

fn assignment_at(items: &[CellItem], order: usize) -> Option<&crate::ir::Assignment> {
    items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(value) => Some(value),
            _ => None,
        })
        .nth(order)
}
fn actual_span(applied: &AppliedTopologyTransform, order: usize, fallback: &Span) -> Span {
    applied
        .provenance
        .get(order)
        .map(|value| value.span().clone())
        .unwrap_or_else(|| fallback.clone())
}
fn expr_operand(expression: &Expr, index: usize) -> Option<String> {
    match expression {
        Expr::Atom(value) if index == 0 => Some(value.clone()),
        Expr::List(values) => values.get(index + 1).and_then(|value| match value {
            Expr::Atom(value) => Some(value.clone()),
            _ => None,
        }),
        _ => None,
    }
}
fn delay_component<'a>(
    constraint: &'a TimingConstraint,
    transition: Transition,
    span: &Span,
) -> Result<&'a AdditiveDelay, TopologyVerificationError> {
    let index = match (transition, constraint.additive_delay().len()) {
        (Transition::Rise, _) => 0,
        (Transition::Fall, 2 | 3) => 1,
        (Transition::TurnOff, 3) => 2,
        _ => {
            return Err(TopologyVerificationError::new(
                span.clone(),
                "transition has no source delay component",
            ));
        }
    };
    constraint.additive_delay().component(index).ok_or_else(|| {
        TopologyVerificationError::new(span.clone(), "source delay component is absent")
    })
}
fn selected_terms(
    tuple: &DelayTuple,
    transition: Transition,
    span: &Span,
) -> Result<Vec<TimingExpr>, TopologyVerificationError> {
    let index = match (transition, tuple.len()) {
        (_, 1) => 0,
        (Transition::Rise, _) => 0,
        (Transition::Fall, 2 | 3) => 1,
        (Transition::TurnOff, 3) => 2,
        _ => {
            return Err(TopologyVerificationError::new(
                span.clone(),
                "local transition has no delay component",
            ));
        }
    };
    let value = match tuple {
        DelayTuple::One(value) => value,
        DelayTuple::Two { rise, fall } => {
            if index == 0 {
                rise
            } else {
                fall
            }
        }
        DelayTuple::Three {
            rise,
            fall,
            turn_off,
        } => match index {
            0 => rise,
            1 => fall,
            _ => turn_off,
        },
    };
    nonzero_terms(
        &AdditiveDelay::from_timing_expr(value.clone())
            .map_err(|error| TopologyVerificationError::new(span.clone(), error.to_string()))?,
        span,
    )
}
fn nonzero_terms(
    delay: &AdditiveDelay,
    _span: &Span,
) -> Result<Vec<TimingExpr>, TopologyVerificationError> {
    let terms = delay
        .terms()
        .iter()
        .map(|term| term.as_timing_expr().clone())
        .collect::<Vec<_>>();
    if terms.len() == 1
        && terms[0]
            == TimingExpr::atom("0").map_err(|error| {
                TopologyVerificationError::new(
                    Span::new("<topology-verify>", 1, 1),
                    error.to_string(),
                )
            })?
    {
        return Ok(Vec::new());
    }
    Ok(terms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::TimingOperator;

    #[test]
    fn one_entry_delay_applies_to_every_transition_and_retains_ordered_zero_terms() {
        let one = DelayTuple::One(TimingExpr::atom("T").unwrap());
        for transition in [Transition::Rise, Transition::Fall, Transition::TurnOff] {
            assert_eq!(
                selected_terms(&one, transition, &Span::new("verify.sv", 1, 1)).unwrap(),
                vec![TimingExpr::atom("T").unwrap()]
            );
        }
        assert!(
            selected_terms(
                &DelayTuple::One(TimingExpr::atom("0").unwrap()),
                Transition::Rise,
                &Span::new("verify.sv", 1, 1),
            )
            .unwrap()
            .is_empty()
        );
        let two = DelayTuple::Two {
            rise: TimingExpr::atom("R").unwrap(),
            fall: TimingExpr::atom("F").unwrap(),
        };
        assert_eq!(
            selected_terms(&two, Transition::Rise, &Span::new("verify.sv", 1, 1)).unwrap(),
            vec![TimingExpr::atom("R").unwrap()]
        );
        assert_eq!(
            selected_terms(&two, Transition::Fall, &Span::new("verify.sv", 1, 1)).unwrap(),
            vec![TimingExpr::atom("F").unwrap()]
        );
        let three = DelayTuple::Three {
            rise: TimingExpr::atom("R").unwrap(),
            fall: TimingExpr::atom("F").unwrap(),
            turn_off: TimingExpr::atom("Z").unwrap(),
        };
        for (transition, term) in [
            (Transition::Rise, "R"),
            (Transition::Fall, "F"),
            (Transition::TurnOff, "Z"),
        ] {
            assert_eq!(
                selected_terms(&three, transition, &Span::new("verify.sv", 1, 1)).unwrap(),
                vec![TimingExpr::atom(term).unwrap()]
            );
        }
        let duplicate = TimingExpr::operation(
            TimingOperator::Add,
            vec![
                TimingExpr::atom("A").unwrap(),
                TimingExpr::atom("0").unwrap(),
                TimingExpr::atom("A").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(
            selected_terms(
                &DelayTuple::One(duplicate),
                Transition::Rise,
                &Span::new("verify.sv", 1, 1),
            )
            .unwrap(),
            vec![
                TimingExpr::atom("A").unwrap(),
                TimingExpr::atom("0").unwrap(),
                TimingExpr::atom("A").unwrap(),
            ]
        );
    }

    #[test]
    fn actual_transition_rule_is_exact_only_for_unate_edges() {
        let span = Span::new("verify.sv", 9, 4);
        let error = verify_actual_transition(
            TimingSense::NegativeUnate,
            Transition::Rise,
            Transition::Rise,
            &span,
        )
        .unwrap_err();
        assert_eq!(error.span(), &span);
        assert!(
            verify_actual_transition(
                TimingSense::Conditional,
                Transition::Rise,
                Transition::Fall,
                &span,
            )
            .is_ok()
        );
        assert!(
            verify_actual_transition(
                TimingSense::NonUnate,
                Transition::Fall,
                Transition::Rise,
                &span,
            )
            .is_ok()
        );
        assert!(
            verify_actual_transition(
                TimingSense::NegativeUnate,
                Transition::TurnOff,
                Transition::Rise,
                &span,
            )
            .is_ok()
        );
    }
}
