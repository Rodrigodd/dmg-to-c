//! Exact application and erasure of one resolved physical topology overlay.
//!
//! This module accepts only a resolved topology boundary. It neither parses
//! TOML nor searches for topology-shaped names.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::diagnostic::Span;
use crate::ir::{Assignment, CellItem, Expr, LoweredModule};
use crate::timing_graph::{
    AssignmentDelayOrigin, AssignmentOrigin, AssignmentProvenance, TimingSignalMetadata,
    TimingSignalRole,
};
use crate::topology_hint::{
    BaselineAssignmentId, HintAssignmentId, HintSignalId, ResolvedBaselineAssignment,
    ResolvedPathRecipe, ResolvedTopologyAssignment, ResolvedTopologyRewrite,
    TopologyMaterializationBoundary, TopologyOperandRef, TopologyValueExpr,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyApplyError {
    span: Span,
    message: String,
}

impl TopologyApplyError {
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

impl fmt::Display for TopologyApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: {}",
            self.span.path.display(),
            self.span.line,
            self.span.column,
            self.message
        )
    }
}

impl Error for TopologyApplyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTopologyAssignment {
    pub id: HintAssignmentId,
    pub item_order: usize,
    pub assignment_order: usize,
    pub assignment: Assignment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTopologyRewrite {
    pub baseline: BaselineAssignmentId,
    pub item_order: usize,
    pub assignment_order: usize,
    pub before: Assignment,
    pub after: Assignment,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppliedTopologyFacts {
    pub assignments: BTreeMap<HintAssignmentId, AppliedTopologyAssignment>,
    pub rewrites: BTreeMap<BaselineAssignmentId, AppliedTopologyRewrite>,
    /// Every original assignment ordinal mapped to its transformed ordinal.
    pub original_assignment_orders: BTreeMap<usize, usize>,
}

/// Exact reversible snapshot boundary. Erasure never recognizes names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyErasure {
    span: Span,
    original_lowered: LoweredModule,
    original_provenance: Vec<AssignmentProvenance>,
    original_metadata: Vec<TimingSignalMetadata>,
    expected_lowered: LoweredModule,
    expected_provenance: Vec<AssignmentProvenance>,
    expected_metadata: Vec<TimingSignalMetadata>,
}

impl TopologyErasure {
    pub fn erase(
        &self,
        transformed: &LoweredModule,
        provenance: &[AssignmentProvenance],
        metadata: &[TimingSignalMetadata],
    ) -> Result<
        (
            LoweredModule,
            Vec<AssignmentProvenance>,
            Vec<TimingSignalMetadata>,
        ),
        TopologyApplyError,
    > {
        if transformed != &self.expected_lowered {
            return Err(TopologyApplyError::new(
                self.span.clone(),
                "transformed lowered module differs from the exact materialized snapshot",
            ));
        }
        if provenance != self.expected_provenance {
            return Err(TopologyApplyError::new(
                self.span.clone(),
                "transformed assignment provenance differs from the exact materialized snapshot",
            ));
        }
        if metadata != self.expected_metadata {
            return Err(TopologyApplyError::new(
                self.span.clone(),
                "transformed timing metadata differs from the exact materialized snapshot",
            ));
        }
        Ok((
            self.original_lowered.clone(),
            self.original_provenance.clone(),
            self.original_metadata.clone(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTopologyTransform {
    pub lowered: LoweredModule,
    pub provenance: Vec<AssignmentProvenance>,
    pub metadata: Vec<TimingSignalMetadata>,
    pub facts: AppliedTopologyFacts,
    pub erasure: TopologyErasure,
}

pub fn materialize_topology(
    boundary: TopologyMaterializationBoundary<'_>,
    lowered: &LoweredModule,
    metadata: &[TimingSignalMetadata],
    provenance: &[AssignmentProvenance],
) -> Result<AppliedTopologyTransform, TopologyApplyError> {
    let hint = boundary.hint();
    validate_baseline_alignment(hint.baseline_assignments(), lowered, provenance)?;
    validate_metadata(metadata)?;
    validate_baseline_metadata(lowered, metadata)?;

    let existing_names = baseline_names(lowered);
    let mut signal_names = BTreeMap::<HintSignalId, String>::new();
    for signal in hint.signals() {
        if existing_names.contains(signal.name()) {
            return Err(TopologyApplyError::new(
                signal.span().clone(),
                format!(
                    "generated topology signal {} collides with the baseline",
                    signal.name()
                ),
            ));
        }
        if metadata.iter().any(|value| value.name() == signal.name()) {
            return Err(TopologyApplyError::new(
                signal.span().clone(),
                format!(
                    "generated topology signal {} collides with existing timing metadata",
                    signal.name()
                ),
            ));
        }
        if signal_names
            .insert(signal.id().clone(), signal.name().to_string())
            .is_some()
        {
            return Err(TopologyApplyError::new(
                signal.span().clone(),
                format!("duplicate resolved topology signal {}", signal.id()),
            ));
        }
    }

    let assignment_by_id = hint
        .assignments()
        .iter()
        .map(|assignment| (assignment.id().clone(), assignment))
        .collect::<BTreeMap<_, _>>();
    validate_generated_drivers(hint.assignments(), hint.signals())?;
    let generated_order = validate_generated_order_and_reachability(
        hint.assignments(),
        hint.rewrites(),
        &assignment_by_id,
    )?;
    validate_recipe_terminals(hint.recipes(), hint.rewrites())?;

    let baseline_by_id = hint
        .baseline_assignments()
        .iter()
        .map(|assignment| (assignment.id().clone(), assignment))
        .collect::<BTreeMap<_, _>>();
    let mut rewrite_by_item = BTreeMap::new();
    for rewrite in hint.rewrites() {
        let baseline = baseline_by_id.get(&rewrite.baseline).ok_or_else(|| {
            TopologyApplyError::new(
                rewrite.span.clone(),
                format!(
                    "resolved rewrite references missing baseline {}",
                    rewrite.baseline
                ),
            )
        })?;
        let replacement = assignment_by_id.get(&rewrite.replacement).ok_or_else(|| {
            TopologyApplyError::new(
                rewrite.span.clone(),
                format!(
                    "resolved rewrite references missing assignment {}",
                    rewrite.replacement
                ),
            )
        })?;
        rewrite_by_item.insert(
            baseline.item_order(),
            (
                rewrite.baseline.clone(),
                replacement.target_name().to_string(),
                rewrite.span.clone(),
            ),
        );
    }
    let earliest_rewrite_item = rewrite_by_item.keys().next().copied().ok_or_else(|| {
        TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "resolved topology contains no rewrites",
        )
    })?;
    validate_baseline_dependency_order(hint.assignments(), lowered, earliest_rewrite_item)?;

    let generated_assignments = generated_order
        .iter()
        .map(|assignment| {
            Ok((
                assignment.id().clone(),
                Assignment {
                    target: assignment.target_name().to_string(),
                    expr: materialize_expression(assignment, &signal_names)?,
                    delay: assignment.delay().clone(),
                },
                generated_parent(
                    assignment.id(),
                    hint.rewrites(),
                    &assignment_by_id,
                    &baseline_by_id,
                    provenance,
                )?,
                assignment.span().clone(),
            ))
        })
        .collect::<Result<Vec<_>, TopologyApplyError>>()?;

    let mut transformed_items = Vec::with_capacity(
        lowered
            .cell
            .items
            .len()
            .saturating_add(generated_assignments.len()),
    );
    let mut transformed_provenance =
        Vec::with_capacity(provenance.len().saturating_add(generated_assignments.len()));
    let mut facts = AppliedTopologyFacts::default();
    let mut original_assignment_order = 0usize;
    let mut generated_inserted = false;

    for (item_order, item) in lowered.cell.items.iter().enumerate() {
        if item_order == earliest_rewrite_item {
            for (id, assignment, parent, span) in &generated_assignments {
                let assignment_order = transformed_provenance.len();
                let generated_item_order = transformed_items.len();
                transformed_items.push(CellItem::Assignment(assignment.clone()));
                transformed_provenance.push(new_provenance(
                    assignment_order,
                    parent.source_assignment_order(),
                    span.clone(),
                    AssignmentOrigin::GeneratedTopology {
                        parent: parent.origin().source(),
                    },
                    AssignmentDelayOrigin::TopologyPlacement,
                    Vec::new(),
                )?);
                facts.assignments.insert(
                    id.clone(),
                    AppliedTopologyAssignment {
                        id: id.clone(),
                        item_order: generated_item_order,
                        assignment_order,
                        assignment: assignment.clone(),
                    },
                );
            }
            generated_inserted = true;
        }
        match item {
            CellItem::Assignment(original) => {
                let original_provenance =
                    provenance.get(original_assignment_order).ok_or_else(|| {
                        TopologyApplyError::new(
                            Span::new("<topology-materialize>", 1, 1),
                            "provenance is shorter than baseline assignment order",
                        )
                    })?;
                let mut after = original.clone();
                if let Some((baseline, replacement_target, _)) = rewrite_by_item.get(&item_order) {
                    after.expr = Expr::atom(replacement_target.clone());
                    facts.rewrites.insert(
                        baseline.clone(),
                        AppliedTopologyRewrite {
                            baseline: baseline.clone(),
                            item_order: transformed_items.len(),
                            assignment_order: transformed_provenance.len(),
                            before: original.clone(),
                            after: after.clone(),
                        },
                    );
                }
                transformed_items.push(CellItem::Assignment(after));
                facts
                    .original_assignment_orders
                    .insert(original_assignment_order, transformed_provenance.len());
                transformed_provenance.push(reindex_provenance(
                    original_provenance,
                    transformed_provenance.len(),
                )?);
                original_assignment_order += 1;
            }
            other => transformed_items.push(other.clone()),
        }
    }
    if !generated_inserted {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "no generated topology insertion point was found",
        ));
    }
    if original_assignment_order != provenance.len() {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "provenance is longer than the baseline assignment list",
        ));
    }
    if facts.original_assignment_orders.len() != provenance.len() {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "original assignment order map is incomplete",
        ));
    }

    let mut transformed_metadata = metadata.to_vec();
    for signal in hint.signals() {
        transformed_metadata.push(
            TimingSignalMetadata::new(
                signal.name().to_string(),
                BTreeSet::from([TimingSignalRole::TopologyTemporary]),
                signal.span().clone(),
            )
            .map_err(|error| TopologyApplyError::new(signal.span().clone(), error.to_string()))?,
        );
    }
    let transformed = LoweredModule {
        cell: crate::ir::Cell {
            items: transformed_items,
            ..lowered.cell.clone()
        },
        timing_aliases: lowered.timing_aliases.clone(),
        diagnostics: lowered.diagnostics.clone(),
    };
    validate_transformed(
        &transformed,
        &transformed_provenance,
        &transformed_metadata,
        metadata,
    )?;

    let erasure = TopologyErasure {
        span: hint
            .rewrites()
            .first()
            .map(|rewrite| rewrite.span.clone())
            .unwrap_or_else(|| Span::new("<topology-erasure>", 1, 1)),
        original_lowered: lowered.clone(),
        original_provenance: provenance.to_vec(),
        original_metadata: metadata.to_vec(),
        expected_lowered: transformed.clone(),
        expected_provenance: transformed_provenance.clone(),
        expected_metadata: transformed_metadata.clone(),
    };
    Ok(AppliedTopologyTransform {
        lowered: transformed,
        provenance: transformed_provenance,
        metadata: transformed_metadata,
        facts,
        erasure,
    })
}

fn validate_baseline_alignment(
    baselines: &[ResolvedBaselineAssignment],
    lowered: &LoweredModule,
    provenance: &[AssignmentProvenance],
) -> Result<(), TopologyApplyError> {
    let assignment_items = lowered
        .cell
        .items
        .iter()
        .enumerate()
        .filter_map(|(item_order, item)| match item {
            CellItem::Assignment(assignment) => Some((item_order, assignment)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if assignment_items.len() != provenance.len() {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "baseline assignment provenance is not aligned with baseline assignments",
        ));
    }
    for (assignment_order, entry) in provenance.iter().enumerate() {
        if entry.assignment_order() != assignment_order {
            return Err(TopologyApplyError::new(
                entry.span().clone(),
                "baseline provenance assignment_order is not aligned",
            ));
        }
    }
    for baseline in baselines {
        let Some((item_order, assignment)) = assignment_items.get(baseline.assignment_order())
        else {
            return Err(TopologyApplyError::new(
                baseline.span().clone(),
                "resolved baseline assignment order is outside the supplied baseline",
            ));
        };
        if *item_order != baseline.item_order()
            || assignment.target != baseline.anchor().target()
            || assignment.expr != baseline.anchor().expression().to_expr()
        {
            return Err(TopologyApplyError::new(
                baseline.span().clone(),
                format!(
                    "resolved baseline assignment {} no longer matches the supplied baseline",
                    baseline.id()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_metadata(metadata: &[TimingSignalMetadata]) -> Result<(), TopologyApplyError> {
    let mut names = BTreeSet::new();
    for value in metadata {
        if !names.insert(value.name()) {
            return Err(TopologyApplyError::new(
                value.span().clone(),
                format!("duplicate timing metadata signal {}", value.name()),
            ));
        }
    }
    Ok(())
}

fn validate_baseline_metadata(
    lowered: &LoweredModule,
    metadata: &[TimingSignalMetadata],
) -> Result<(), TopologyApplyError> {
    let baseline_names = baseline_names(lowered);
    for value in metadata {
        if !baseline_names.contains(value.name()) {
            return Err(TopologyApplyError::new(
                value.span().clone(),
                format!(
                    "baseline timing metadata names non-baseline signal {}",
                    value.name()
                ),
            ));
        }
    }
    Ok(())
}

fn baseline_names(lowered: &LoweredModule) -> BTreeSet<String> {
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
    names.extend(lowered.cell.items.iter().filter_map(|item| match item {
        CellItem::Assignment(assignment) => Some(assignment.target.clone()),
        _ => None,
    }));
    names
}

fn validate_baseline_dependency_order(
    assignments: &[ResolvedTopologyAssignment],
    lowered: &LoweredModule,
    insertion_item_order: usize,
) -> Result<(), TopologyApplyError> {
    let inputs = lowered.cell.inputs.iter().collect::<BTreeSet<_>>();
    let registers = lowered
        .cell
        .registers
        .iter()
        .map(|register| &register.name)
        .collect::<BTreeSet<_>>();
    let mut drivers = BTreeMap::<&str, Vec<usize>>::new();
    for (item_order, item) in lowered.cell.items.iter().enumerate() {
        if let CellItem::Assignment(assignment) = item {
            drivers
                .entry(assignment.target.as_str())
                .or_default()
                .push(item_order);
        }
    }
    for assignment in assignments {
        for operand in assignment.operands() {
            let TopologyOperandRef::BaselineSignal(signal) = operand else {
                continue;
            };
            if inputs.contains(signal) || registers.contains(signal) {
                continue;
            }
            let Some(driver_orders) = drivers.get(signal.as_str()) else {
                return Err(TopologyApplyError::new(
                    assignment.span().clone(),
                    format!("baseline operand {} has no baseline driver", signal),
                ));
            };
            if driver_orders
                .iter()
                .any(|driver_order| *driver_order >= insertion_item_order)
            {
                return Err(TopologyApplyError::new(
                    assignment.span().clone(),
                    format!(
                        "generated assignment {} depends on ordinary baseline signal {} after the topology insertion point",
                        assignment.id(),
                        signal
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_generated_drivers(
    assignments: &[ResolvedTopologyAssignment],
    signals: &[crate::topology_hint::ResolvedTopologySignal],
) -> Result<(), TopologyApplyError> {
    let mut driver_counts = BTreeMap::<HintSignalId, usize>::new();
    for assignment in assignments {
        *driver_counts
            .entry(assignment.target().clone())
            .or_default() += 1;
    }
    for signal in signals {
        if driver_counts.get(signal.id()) != Some(&1) {
            return Err(TopologyApplyError::new(
                signal.span().clone(),
                format!(
                    "generated topology signal {} must have exactly one generated driver",
                    signal.id()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_generated_order_and_reachability<'a>(
    assignments: &'a [ResolvedTopologyAssignment],
    rewrites: &[ResolvedTopologyRewrite],
    by_id: &BTreeMap<HintAssignmentId, &'a ResolvedTopologyAssignment>,
) -> Result<Vec<&'a ResolvedTopologyAssignment>, TopologyApplyError> {
    let mut declared = BTreeSet::new();
    for assignment in assignments {
        for operand in assignment.operands() {
            if let TopologyOperandRef::GeneratedSignal(signal) = operand {
                let producer = assignments
                    .iter()
                    .find(|candidate| candidate.target() == signal)
                    .ok_or_else(|| {
                        TopologyApplyError::new(
                            assignment.span().clone(),
                            format!(
                                "generated operand signal {} has no declared generated driver",
                                signal
                            ),
                        )
                    })?;
                if !declared.contains(producer.id()) {
                    return Err(TopologyApplyError::new(
                        assignment.span().clone(),
                        format!(
                            "generated assignment {} has a forward dependency on {}",
                            assignment.id(),
                            producer.id()
                        ),
                    ));
                }
            }
        }
        declared.insert(assignment.id().clone());
    }

    let mut reachable = BTreeSet::new();
    let mut pending = rewrites
        .iter()
        .map(|rewrite| rewrite.replacement.clone())
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        let assignment = by_id.get(&id).ok_or_else(|| {
            TopologyApplyError::new(
                Span::new("<topology-materialize>", 1, 1),
                format!(
                    "rewrite replacement {} is not a resolved generated assignment",
                    id
                ),
            )
        })?;
        for operand in assignment.operands() {
            if let TopologyOperandRef::GeneratedSignal(signal) = operand {
                let producer = assignments
                    .iter()
                    .find(|candidate| candidate.target() == signal)
                    .ok_or_else(|| {
                        TopologyApplyError::new(
                            assignment.span().clone(),
                            format!(
                                "generated operand signal {} has no declared generated driver",
                                signal
                            ),
                        )
                    })?;
                pending.push(producer.id().clone());
            }
        }
    }
    for assignment in assignments {
        if !reachable.contains(assignment.id()) {
            return Err(TopologyApplyError::new(
                assignment.span().clone(),
                format!(
                    "generated topology assignment {} is dead outside every rewrite cone",
                    assignment.id()
                ),
            ));
        }
    }
    Ok(assignments.iter().collect())
}

fn validate_recipe_terminals(
    recipes: &[ResolvedPathRecipe],
    rewrites: &[ResolvedTopologyRewrite],
) -> Result<(), TopologyApplyError> {
    for recipe in recipes {
        if let Some(crate::topology_hint::ResolvedPathStepKind::Rewrite(id)) =
            recipe.steps.last().map(|step| &step.kind)
        {
            if !rewrites.iter().any(|rewrite| rewrite.baseline == *id) {
                return Err(TopologyApplyError::new(
                    Span::new("<topology-materialize>", 1, 1),
                    format!(
                        "resolved path recipe {} terminates at a missing rewrite {}",
                        recipe.id, id
                    ),
                ));
            }
        } else {
            return Err(TopologyApplyError::new(
                Span::new("<topology-materialize>", 1, 1),
                format!(
                    "resolved path recipe {} lacks a virtual rewrite terminal",
                    recipe.id
                ),
            ));
        }
    }
    Ok(())
}

fn materialize_expression(
    assignment: &ResolvedTopologyAssignment,
    signal_names: &BTreeMap<HintSignalId, String>,
) -> Result<Expr, TopologyApplyError> {
    let operands = assignment
        .operands()
        .iter()
        .map(|operand| match operand {
            TopologyOperandRef::BaselineSignal(name) => Ok(Expr::atom(name.clone())),
            TopologyOperandRef::GeneratedSignal(id) => signal_names
                .get(id)
                .cloned()
                .map(Expr::atom)
                .ok_or_else(|| {
                    TopologyApplyError::new(
                        assignment.span().clone(),
                        format!(
                            "resolved expression references missing generated signal {}",
                            id
                        ),
                    )
                }),
            TopologyOperandRef::LogicAtom(value) => Ok(Expr::atom(value.as_str())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    match assignment.expression() {
        TopologyValueExpr::Atom(_) => match operands.as_slice() {
            [atom] => Ok(atom.clone()),
            _ => Err(TopologyApplyError::new(
                assignment.span().clone(),
                "resolved atom expression has an invalid operand count",
            )),
        },
        TopologyValueExpr::Operation { operator, .. } => Ok(Expr::value(*operator, operands)),
    }
}

fn generated_parent(
    assignment_id: &HintAssignmentId,
    rewrites: &[ResolvedTopologyRewrite],
    assignments: &BTreeMap<HintAssignmentId, &ResolvedTopologyAssignment>,
    baselines: &BTreeMap<BaselineAssignmentId, &ResolvedBaselineAssignment>,
    provenance: &[AssignmentProvenance],
) -> Result<AssignmentProvenance, TopologyApplyError> {
    let mut candidates = Vec::new();
    for rewrite in rewrites {
        if rewrite_cone_contains(assignment_id, &rewrite.replacement, assignments)? {
            let baseline = baselines.get(&rewrite.baseline).ok_or_else(|| {
                TopologyApplyError::new(
                    rewrite.span.clone(),
                    format!(
                        "resolved rewrite references missing baseline {}",
                        rewrite.baseline
                    ),
                )
            })?;
            let provenance = provenance.get(baseline.assignment_order()).ok_or_else(|| {
                TopologyApplyError::new(
                    baseline.span().clone(),
                    "baseline provenance is missing the rewrite parent assignment",
                )
            })?;
            candidates.push((
                provenance.source_assignment_order(),
                baseline.assignment_order(),
                provenance,
            ));
        }
    }
    candidates
        .into_iter()
        .min_by_key(|(source_order, assignment_order, _)| (*source_order, *assignment_order))
        .map(|(_, _, provenance)| provenance.clone())
        .ok_or_else(|| {
            TopologyApplyError::new(
                Span::new("<topology-materialize>", 1, 1),
                format!(
                    "generated topology assignment {} is outside every rewrite cone",
                    assignment_id
                ),
            )
        })
}

fn rewrite_cone_contains(
    needle: &HintAssignmentId,
    root: &HintAssignmentId,
    assignments: &BTreeMap<HintAssignmentId, &ResolvedTopologyAssignment>,
) -> Result<bool, TopologyApplyError> {
    let mut pending = vec![root.clone()];
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        if &id == needle {
            return Ok(true);
        }
        let assignment = assignments.get(&id).ok_or_else(|| {
            TopologyApplyError::new(
                Span::new("<topology-materialize>", 1, 1),
                format!("resolved rewrite cone references missing assignment {}", id),
            )
        })?;
        for operand in assignment.operands() {
            if let TopologyOperandRef::GeneratedSignal(signal) = operand {
                let producer = assignments
                    .values()
                    .find(|candidate| candidate.target() == signal)
                    .ok_or_else(|| {
                        TopologyApplyError::new(
                            assignment.span().clone(),
                            format!(
                                "generated operand signal {} has no declared generated driver",
                                signal
                            ),
                        )
                    })?;
                pending.push(producer.id().clone());
            }
        }
    }
    Ok(false)
}

fn reindex_provenance(
    value: &AssignmentProvenance,
    assignment_order: usize,
) -> Result<AssignmentProvenance, TopologyApplyError> {
    new_provenance(
        assignment_order,
        value.source_assignment_order(),
        value.span().clone(),
        value.origin(),
        value.delay_origin(),
        value.state_controls().to_vec(),
    )
}

fn new_provenance(
    assignment_order: usize,
    source_assignment_order: usize,
    span: Span,
    origin: AssignmentOrigin,
    delay_origin: AssignmentDelayOrigin,
    state_controls: Vec<crate::timing_graph::StateControlProvenance>,
) -> Result<AssignmentProvenance, TopologyApplyError> {
    AssignmentProvenance::new_with_delay_origin(
        assignment_order,
        source_assignment_order,
        span.clone(),
        origin,
        delay_origin,
        state_controls,
    )
    .map_err(|error| TopologyApplyError::new(span, error.to_string()))
}

fn validate_transformed(
    lowered: &LoweredModule,
    provenance: &[AssignmentProvenance],
    metadata: &[TimingSignalMetadata],
    original_metadata: &[TimingSignalMetadata],
) -> Result<(), TopologyApplyError> {
    lowered.cell.validate().map_err(|error| {
        TopologyApplyError::new(Span::new("<topology-materialize>", 1, 1), error.to_string())
    })?;
    if metadata.len() < original_metadata.len()
        || metadata[..original_metadata.len()] != *original_metadata
    {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "original timing metadata is not an exact transformed prefix",
        ));
    }
    let assignments = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            _ => None,
        })
        .collect::<Vec<_>>();
    if assignments.len() != provenance.len() {
        return Err(TopologyApplyError::new(
            Span::new("<topology-materialize>", 1, 1),
            "transformed assignment provenance is not aligned",
        ));
    }
    for (order, (_assignment, provenance)) in assignments.iter().zip(provenance).enumerate() {
        if provenance.assignment_order() != order {
            return Err(TopologyApplyError::new(
                provenance.span().clone(),
                "transformed provenance assignment_order is not aligned",
            ));
        }
        if provenance.origin().is_topology_generated() && !provenance.state_controls().is_empty() {
            return Err(TopologyApplyError::new(
                provenance.span().clone(),
                "generated topology assignment unexpectedly carries state controls",
            ));
        }
    }
    for metadata in &metadata[original_metadata.len()..] {
        if !metadata
            .roles()
            .contains(&TimingSignalRole::TopologyTemporary)
            || metadata.roles().len() != 1
        {
            return Err(TopologyApplyError::new(
                metadata.span().clone(),
                "generated topology metadata has an invalid role set",
            ));
        }
        let matching = assignments
            .iter()
            .zip(provenance)
            .filter(|(assignment, provenance)| {
                assignment.target == metadata.name() && provenance.origin().is_topology_generated()
            })
            .count();
        if matching != 1 {
            return Err(TopologyApplyError::new(
                metadata.span().clone(),
                format!(
                    "generated topology metadata {} does not have exactly one generated driver",
                    metadata.name()
                ),
            ));
        }
    }
    validate_metadata(metadata)
}
