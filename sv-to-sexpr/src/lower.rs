use crate::analyze::{
    ModuleCatalog, analyze_design_structural, analyze_design_with_catalog_and_generate_mode,
    resolve_keeper_ast_instantiation, sensitivity_is_stateful,
};
use crate::ast::*;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Span};
use crate::elaborate::{GenerateMode, elaborate_design};
use crate::ir::{
    Assignment, Cell, CellItem, DelayTuple, Expr, LogicValue, LoweredModule, Register,
    StrengthPair, TimingExpr, TimingOperator, ValueOperator,
};
use crate::timing_apply::{
    ActualDecompositionVerification, AppliedTimingFacts, TimingErasure, apply_decomposition,
};
use crate::timing_decompose::{Decomposition, decompose_timing};
use crate::timing_graph::{
    AssignmentDelayOrigin, AssignmentOrigin, AssignmentProvenance, CutTimingGraph,
    SourceAssignmentOrigin, StateControlProvenance, TimingAnalysisReport, TimingConstraintSource,
    TimingControlSource, TimingGraph, TimingSignalMetadata, TimingSignalRole, Transition,
    analyze_timing_graph, build_timing_graph, cut_register_cycles,
};
use crate::topology_apply::{AppliedTopologyFacts, TopologyErasure, materialize_topology};
use crate::topology_hint::{TopologyHintContext, builtin_topology_hint_catalog};
use crate::topology_verify::{ActualTopologyVerification, verify_materialized_topology};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub type LowerResult<T> = Result<T, Diagnostic>;
type SvExpr = crate::ast::Expr;

#[derive(Debug, Clone)]
pub struct LoweredTimingModel {
    lowered: LoweredModule,
    assignment_provenance: Vec<AssignmentProvenance>,
    functional_graph: TimingGraph,
    cut_graph: CutTimingGraph,
    timing_analysis: TimingAnalysisReport,
}

impl LoweredTimingModel {
    pub fn lowered(&self) -> &LoweredModule {
        &self.lowered
    }

    pub fn assignment_provenance(&self) -> &[AssignmentProvenance] {
        &self.assignment_provenance
    }

    pub fn functional_graph(&self) -> &TimingGraph {
        &self.functional_graph
    }

    pub fn cut_graph(&self) -> &CutTimingGraph {
        &self.cut_graph
    }

    pub fn timing_analysis(&self) -> &TimingAnalysisReport {
        &self.timing_analysis
    }

    pub fn into_lowered(self) -> LoweredModule {
        self.lowered
    }
}

#[derive(Debug, Clone)]
pub struct LoweredDecomposedTimingModel {
    lowered: LoweredModule,
    strategy: DecomposedTimingStrategy,
    assignment_provenance: Vec<AssignmentProvenance>,
    signal_metadata: Vec<TimingSignalMetadata>,
    functional_graph: TimingGraph,
    cut_graph: CutTimingGraph,
    timing_analysis: TimingAnalysisReport,
    erasure: DecomposedTimingErasure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecomposedTimingStrategy {
    ExactCover {
        decomposition: Decomposition,
        applied_facts: AppliedTimingFacts,
        actual_verification: ActualDecompositionVerification,
    },
    PhysicalTopology {
        module: String,
        applied_facts: AppliedTopologyFacts,
        actual_verification: ActualTopologyVerification,
    },
}
impl DecomposedTimingStrategy {
    pub fn exact_cover(
        &self,
    ) -> Option<(
        &Decomposition,
        &AppliedTimingFacts,
        &ActualDecompositionVerification,
    )> {
        match self {
            Self::ExactCover {
                decomposition,
                applied_facts,
                actual_verification,
            } => Some((decomposition, applied_facts, actual_verification)),
            _ => None,
        }
    }
    pub fn physical_topology(
        &self,
    ) -> Option<(&str, &AppliedTopologyFacts, &ActualTopologyVerification)> {
        match self {
            Self::PhysicalTopology {
                module,
                applied_facts,
                actual_verification,
            } => Some((module, applied_facts, actual_verification)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecomposedTimingErasure {
    ExactCover(TimingErasure),
    PhysicalTopology {
        erasure: TopologyErasure,
        metadata: Vec<TimingSignalMetadata>,
    },
}

#[derive(Debug, Clone)]
pub struct DecomposedTimingErasureError {
    span: Span,
    message: String,
}
impl DecomposedTimingErasure {
    pub fn erase(
        &self,
        lowered: &LoweredModule,
        provenance: &[AssignmentProvenance],
    ) -> Result<crate::timing_apply::ErasedTimingModel, DecomposedTimingErasureError> {
        match self {
            Self::ExactCover(erasure) => {
                erasure
                    .erase(lowered, provenance)
                    .map_err(|error| DecomposedTimingErasureError {
                        span: error.span().clone(),
                        message: error.to_string(),
                    })
            }
            Self::PhysicalTopology { erasure, metadata } => erasure
                .erase(lowered, provenance, metadata)
                .map(|(lowered, provenance, metadata)| {
                    crate::timing_apply::ErasedTimingModel::from_parts(
                        lowered, provenance, metadata,
                    )
                })
                .map_err(|error| DecomposedTimingErasureError {
                    span: error.span().clone(),
                    message: error.to_string(),
                }),
        }
    }
}
impl DecomposedTimingErasureError {
    pub fn span(&self) -> &Span {
        &self.span
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl std::fmt::Display for DecomposedTimingErasureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
impl std::error::Error for DecomposedTimingErasureError {}

impl LoweredDecomposedTimingModel {
    pub fn lowered(&self) -> &LoweredModule {
        &self.lowered
    }

    pub fn strategy(&self) -> &DecomposedTimingStrategy {
        &self.strategy
    }
    pub fn decomposition(&self) -> Option<&Decomposition> {
        match &self.strategy {
            DecomposedTimingStrategy::ExactCover { decomposition, .. } => Some(decomposition),
            _ => None,
        }
    }
    pub fn is_physical_topology(&self) -> bool {
        matches!(
            &self.strategy,
            DecomposedTimingStrategy::PhysicalTopology { .. }
        )
    }

    pub fn assignment_provenance(&self) -> &[AssignmentProvenance] {
        &self.assignment_provenance
    }
    pub fn signal_metadata(&self) -> &[TimingSignalMetadata] {
        &self.signal_metadata
    }

    pub fn functional_graph(&self) -> &TimingGraph {
        &self.functional_graph
    }

    pub fn cut_graph(&self) -> &CutTimingGraph {
        &self.cut_graph
    }

    pub fn timing_analysis(&self) -> &TimingAnalysisReport {
        &self.timing_analysis
    }

    pub fn erasure(&self) -> &DecomposedTimingErasure {
        &self.erasure
    }

    pub fn into_lowered(self) -> LoweredModule {
        self.lowered
    }
}

#[derive(Debug, Clone)]
struct LoweringArtifacts {
    lowered: LoweredModule,
    signal_metadata: Vec<TimingSignalMetadata>,
    assignment_provenance: Vec<AssignmentProvenance>,
    timing_constraint_sources: Vec<TimingConstraintSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimingLoweringPolicy {
    LegacyFirst,
    DecompositionBaseline,
}

pub fn lower_file(path: &Path, input: &str) -> LowerResult<LoweredModule> {
    lower_file_with_generate_mode(path, input, GenerateMode::default())
}

pub fn lower_file_with_generate_mode(
    path: &Path,
    input: &str,
    mode: GenerateMode,
) -> LowerResult<LoweredModule> {
    let design = crate::parser::parse_file(path, input)?;
    lower_design_with_generate_mode(&design, mode)
}

pub fn lower_file_with_catalog_and_generate_mode(
    path: &Path,
    input: &str,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> LowerResult<LoweredModule> {
    let design = crate::parser::parse_file(path, input)?;
    lower_design_with_catalog_and_generate_mode(&design, catalog, mode)
}

pub fn lower_file_with_catalog(
    path: &Path,
    input: &str,
    catalog: &ModuleCatalog,
) -> LowerResult<LoweredModule> {
    lower_file_with_catalog_and_generate_mode(path, input, catalog, GenerateMode::default())
}

/// Lowers the unelaborated M3 structural view.
///
/// This entrypoint exists for milestone inventory tests that must continue to
/// observe an unresolved generate as a lowering deferral. Configured conversion
/// code should use [`lower_file`] or [`lower_file_with_generate_mode`].
pub fn lower_file_structural(path: &Path, input: &str) -> LowerResult<LoweredModule> {
    let design = crate::parser::parse_file(path, input)?;
    let analysis = analyze_design_structural(&design);
    lower_elaborated_design(&design, &analysis)
}

pub fn lower_design_with_generate_mode(
    design: &Design,
    mode: GenerateMode,
) -> LowerResult<LoweredModule> {
    let elaborated = elaborate_design(design, mode)?;
    let analysis = analyze_design_structural(&elaborated);
    lower_elaborated_design(&elaborated, &analysis)
}

pub fn lower_design_with_catalog_and_generate_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> LowerResult<LoweredModule> {
    // Preserve catalog-aware analysis as the pre-flattened record and validate
    // bindings before the hierarchy transform consumes them.
    let configured = analyze_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    if let Some(diagnostic) = configured
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::Error)
    {
        return Err(diagnostic.clone());
    }
    let flattened =
        crate::hierarchy::flatten_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    let analysis = analyze_design_structural(&flattened);
    lower_elaborated_design(&flattened, &analysis)
}

pub fn lower_design_with_catalog(
    design: &Design,
    catalog: &ModuleCatalog,
) -> LowerResult<LoweredModule> {
    lower_design_with_catalog_and_generate_mode(design, catalog, GenerateMode::default())
}

/// Configured timing-aware lowering without a sibling hierarchy catalog.
pub fn lower_design_with_timing_and_generate_mode(
    design: &Design,
    mode: GenerateMode,
) -> LowerResult<LoweredTimingModel> {
    let elaborated = elaborate_design(design, mode)?;
    let analysis = analyze_design_structural(&elaborated);
    lower_timing_model_for_elaborated_design(&elaborated, &analysis)
}

/// Configured timing-aware lowering with ordinary hierarchy flattening.
pub fn lower_design_with_timing_and_catalog_and_generate_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> LowerResult<LoweredTimingModel> {
    let configured = analyze_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    if let Some(diagnostic) = configured
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::Error)
    {
        return Err(diagnostic.clone());
    }
    let flattened =
        crate::hierarchy::flatten_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    let analysis = analyze_design_structural(&flattened);
    lower_timing_model_for_elaborated_design(&flattened, &analysis)
}

/// Opt-in lowering which replaces specify fallback delays with an exact
/// decomposition across assignments and typed timing identities.
pub fn lower_design_with_decomposed_timing_and_generate_mode(
    design: &Design,
    mode: GenerateMode,
) -> LowerResult<LoweredDecomposedTimingModel> {
    let elaborated = elaborate_design(design, mode)?;
    let analysis = analyze_design_structural(&elaborated);
    lower_decomposed_timing_model_for_elaborated_design(&elaborated, &analysis, mode)
}

/// Catalog-aware opt-in timing decomposition after ordinary hierarchy
/// flattening.
pub fn lower_design_with_decomposed_timing_and_catalog_and_generate_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> LowerResult<LoweredDecomposedTimingModel> {
    let configured = analyze_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    if let Some(diagnostic) = configured
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::Error)
    {
        return Err(diagnostic.clone());
    }
    let flattened =
        crate::hierarchy::flatten_design_with_catalog_and_generate_mode(design, catalog, mode)?;
    let analysis = analyze_design_structural(&flattened);
    lower_decomposed_timing_model_for_elaborated_design(&flattened, &analysis, mode)
}

/// Lowers a design that has already had its generate configuration selected.
/// The supplied analysis must describe that exact elaborated design.
pub fn lower_elaborated_design(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
) -> LowerResult<LoweredModule> {
    Ok(lower_elaborated_design_artifacts(design, analysis)?.lowered)
}

fn lower_timing_model_for_elaborated_design(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
) -> LowerResult<LoweredTimingModel> {
    let artifacts = lower_elaborated_design_artifacts(design, analysis)?;
    let functional_graph = build_timing_graph(
        &artifacts.lowered.cell,
        &artifacts.signal_metadata,
        &artifacts.assignment_provenance,
        &artifacts.timing_constraint_sources,
    )?;
    let cut_graph = cut_register_cycles(&functional_graph)?;
    let timing_analysis = analyze_timing_graph(&functional_graph, &cut_graph)?;
    Ok(LoweredTimingModel {
        lowered: artifacts.lowered,
        assignment_provenance: artifacts.assignment_provenance,
        functional_graph,
        cut_graph,
        timing_analysis,
    })
}

fn lower_decomposed_timing_model_for_elaborated_design(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
    mode: GenerateMode,
) -> LowerResult<LoweredDecomposedTimingModel> {
    let artifacts = lower_elaborated_design_artifacts_with_policy(
        design,
        analysis,
        TimingLoweringPolicy::DecompositionBaseline,
    )?;
    let original_graph = build_timing_graph(
        &artifacts.lowered.cell,
        &artifacts.signal_metadata,
        &artifacts.assignment_provenance,
        &artifacts.timing_constraint_sources,
    )?;
    let original_cut_graph = cut_register_cycles(&original_graph)?;
    let original_analysis = analyze_timing_graph(&original_graph, &original_cut_graph)?;
    let catalog = builtin_topology_hint_catalog()
        .map_err(|error| Diagnostic::error(error.span().clone(), error.to_string()))?;
    let resolved = catalog
        .resolve_optional(&TopologyHintContext::new(
            &artifacts.lowered.cell.name,
            mode,
            &artifacts.lowered,
            &original_graph,
        ))
        .map_err(|error| Diagnostic::error(error.span().clone(), error.to_string()))?;
    if let Some(resolved) = resolved {
        let hint = resolved.hints().first().ok_or_else(|| {
            Diagnostic::error(
                Span::new("<topology-hint>", 1, 1),
                "optional topology resolution returned no hint",
            )
        })?;
        let transformed = materialize_topology(
            hint.require_materialization(),
            &artifacts.lowered,
            &artifacts.signal_metadata,
            &artifacts.assignment_provenance,
        )
        .map_err(|error| Diagnostic::error(error.span().clone(), error.to_string()))?;
        let constraint_sources = original_graph
            .constraints()
            .iter()
            .map(TimingConstraintSource::from_constraint)
            .collect::<Vec<_>>();
        let functional_graph = build_timing_graph(
            &transformed.lowered.cell,
            &transformed.metadata,
            &transformed.provenance,
            &constraint_sources,
        )?;
        let cut_graph = cut_register_cycles(&functional_graph)?;
        let timing_analysis = analyze_timing_graph(&functional_graph, &cut_graph)?;
        let actual_verification =
            verify_materialized_topology(hint, &transformed, &functional_graph)
                .map_err(|error| Diagnostic::error(error.span().clone(), error.to_string()))?;
        return Ok(LoweredDecomposedTimingModel {
            lowered: transformed.lowered,
            strategy: DecomposedTimingStrategy::PhysicalTopology {
                module: hint.module().to_string(),
                applied_facts: transformed.facts,
                actual_verification,
            },
            assignment_provenance: transformed.provenance,
            signal_metadata: transformed.metadata.clone(),
            functional_graph,
            cut_graph,
            timing_analysis,
            erasure: DecomposedTimingErasure::PhysicalTopology {
                erasure: transformed.erasure,
                metadata: transformed.metadata,
            },
        });
    }
    let decomposition = decompose_timing(&original_graph, &original_cut_graph, &original_analysis)
        .map_err(decomposition_diagnostic)?;
    let applied = apply_decomposition(
        &artifacts.lowered,
        &artifacts.signal_metadata,
        &artifacts.assignment_provenance,
        &original_graph,
        &decomposition,
    )
    .map_err(decomposition_diagnostic)?;
    let (
        lowered,
        assignment_provenance,
        signal_metadata,
        applied_facts,
        actual_verification,
        erasure,
    ) = applied.into_parts();
    let constraint_sources = original_graph
        .constraints()
        .iter()
        .map(TimingConstraintSource::from_constraint)
        .collect::<Vec<_>>();
    let functional_graph = build_timing_graph(
        &lowered.cell,
        &signal_metadata,
        &assignment_provenance,
        &constraint_sources,
    )?;
    let cut_graph = cut_register_cycles(&functional_graph)?;
    let timing_analysis = analyze_timing_graph(&functional_graph, &cut_graph)?;
    Ok(LoweredDecomposedTimingModel {
        lowered,
        strategy: DecomposedTimingStrategy::ExactCover {
            decomposition,
            applied_facts,
            actual_verification,
        },
        assignment_provenance,
        signal_metadata,
        functional_graph,
        cut_graph,
        timing_analysis,
        erasure: DecomposedTimingErasure::ExactCover(erasure),
    })
}

fn decomposition_diagnostic(error: crate::timing_decompose::DecompositionError) -> Diagnostic {
    Diagnostic::error(error.span().clone(), error.to_string())
}

fn lower_elaborated_design_artifacts(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
) -> LowerResult<LoweringArtifacts> {
    lower_elaborated_design_artifacts_with_policy(
        design,
        analysis,
        TimingLoweringPolicy::LegacyFirst,
    )
}

fn lower_elaborated_design_artifacts_with_policy(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
    timing_policy: TimingLoweringPolicy,
) -> LowerResult<LoweringArtifacts> {
    if let Some(diagnostic) = analysis
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::Error)
    {
        return Err(diagnostic.clone());
    }
    let module = design
        .first_module()
        .ok_or_else(|| Diagnostic::new(Span::new("<lower>", 1, 1), "expected one module"))?;
    let module_analysis = analysis.modules.first().ok_or_else(|| {
        Diagnostic::new(Span::new("<lower>", 1, 1), "expected one analysis module")
    })?;
    lower_module_artifacts(module, module_analysis, timing_policy)
}

fn lower_module_artifacts(
    module: &Module,
    analysis: &crate::analyze::ModuleAnalysis,
    timing_policy: TimingLoweringPolicy,
) -> LowerResult<LoweringArtifacts> {
    let mut lowerer = Lowerer::new(module, analysis, timing_policy)?;
    lowerer.lower_module()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProceduralMode {
    Combinational,
    Stateful,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProceduralContext {
    mode: ProceduralMode,
    condition: Option<Expr>,
    state_controls: Vec<StateControlProvenance>,
}

impl ProceduralContext {
    fn combinational() -> Self {
        Self {
            mode: ProceduralMode::Combinational,
            condition: None,
            state_controls: Vec::new(),
        }
    }

    fn stateful(condition: Option<Expr>, state_controls: Vec<StateControlProvenance>) -> Self {
        Self {
            mode: ProceduralMode::Stateful,
            condition,
            state_controls,
        }
    }
}

#[derive(Debug, Clone)]
struct SourceEmission {
    assignment_span: Span,
    expression_span: Span,
    origin: SourceAssignmentOrigin,
    state_controls: Vec<StateControlProvenance>,
}

impl SourceEmission {
    fn new(
        assignment_span: &Span,
        expression_span: &Span,
        origin: SourceAssignmentOrigin,
        state_controls: Vec<StateControlProvenance>,
    ) -> Self {
        Self {
            assignment_span: assignment_span.clone(),
            expression_span: expression_span.clone(),
            origin,
            state_controls,
        }
    }
}

struct PendingAssignment {
    target: String,
    expr: Expr,
    delay: DelayTuple,
    source_assignment_order: usize,
    diagnostic_span: Span,
    provenance_span: Span,
    origin: AssignmentOrigin,
    delay_origin: AssignmentDelayOrigin,
    state_controls: Vec<StateControlProvenance>,
}

#[derive(Debug, Clone)]
struct SelectedDelay {
    tuple: DelayTuple,
    origin: AssignmentDelayOrigin,
}

struct Lowerer<'a> {
    module: &'a Module,
    cell: Cell,
    timing_alias_sources: BTreeMap<String, TimingAliasSource>,
    timing_aliases: BTreeMap<String, TimingExpr>,
    timing_alias_stack: Vec<String>,
    specify_delays: BTreeMap<String, Vec<SpecifyDelay>>,
    ignored_additional_specify_targets: BTreeSet<String>,
    initialized_registers: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    reserved_names: BTreeSet<String>,
    signal_names: BTreeSet<String>,
    signal_spans: BTreeMap<String, Span>,
    signal_metadata: Vec<TimingSignalMetadata>,
    assignment_provenance: Vec<AssignmentProvenance>,
    timing_constraint_sources: Vec<TimingConstraintSource>,
    next_source_assignment_order: usize,
    next_temp_index: usize,
    timing_policy: TimingLoweringPolicy,
}

#[derive(Debug, Clone)]
struct TimingAliasSource {
    span: Span,
    value: SvExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpecifyDelay {
    path_span: Span,
    delay: DelayTuple,
}

impl<'a> Lowerer<'a> {
    fn new(
        module: &'a Module,
        analysis: &crate::analyze::ModuleAnalysis,
        timing_policy: TimingLoweringPolicy,
    ) -> LowerResult<Self> {
        let signal_names = analysis
            .symbols
            .iter()
            .filter(|(_, symbol)| {
                matches!(
                    symbol.category,
                    crate::analyze::SymbolCategory::Port
                        | crate::analyze::SymbolCategory::Declaration
                )
            })
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let signal_spans = analysis
            .symbols
            .iter()
            .filter(|(_, symbol)| {
                matches!(
                    symbol.category,
                    crate::analyze::SymbolCategory::Port
                        | crate::analyze::SymbolCategory::Declaration
                )
            })
            .map(|(name, symbol)| (name.clone(), symbol.span.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut reserved_names = analysis.symbols.keys().cloned().collect::<BTreeSet<_>>();
        reserved_names.extend(analysis.parameters.keys().cloned());
        reserved_names.extend(analysis.declarations.keys().cloned());
        reserved_names.extend(analysis.localparams.keys().cloned());
        reserved_names.extend(analysis.specparams.keys().cloned());
        reserved_names.extend(analysis.inputs.iter().cloned());
        reserved_names.extend(analysis.outputs.iter().cloned());
        reserved_names.extend(analysis.registers.iter().cloned());
        Ok(Self {
            module,
            cell: Cell {
                name: module.name.clone(),
                inputs: analysis.inputs.clone(),
                outputs: analysis.outputs.clone(),
                registers: analysis
                    .registers
                    .iter()
                    .map(|name| Register {
                        name: name.clone(),
                        initial: LogicValue::X,
                    })
                    .collect(),
                items: Vec::new(),
            },
            timing_alias_sources: BTreeMap::new(),
            timing_aliases: BTreeMap::new(),
            timing_alias_stack: Vec::new(),
            specify_delays: BTreeMap::new(),
            ignored_additional_specify_targets: BTreeSet::new(),
            initialized_registers: BTreeSet::new(),
            diagnostics: Vec::new(),
            reserved_names,
            signal_names,
            signal_spans,
            signal_metadata: timing_signal_metadata(analysis)?,
            assignment_provenance: Vec::new(),
            timing_constraint_sources: Vec::new(),
            next_source_assignment_order: 0,
            next_temp_index: 0,
            timing_policy,
        })
    }

    fn lower_module(&mut self) -> LowerResult<LoweringArtifacts> {
        self.collect_timing_aliases()?;
        self.collect_specify_delays()?;
        for item in &self.module.items {
            self.lower_item(item)?;
        }

        self.cell.validate().map_err(|error| {
            Diagnostic::new(
                self.module.span.clone(),
                format!("invalid lowered cell: {error}"),
            )
        })?;

        self.diagnostics.sort_by(|left, right| {
            left.span
                .path
                .cmp(&right.span.path)
                .then_with(|| left.span.line.cmp(&right.span.line))
                .then_with(|| left.span.column.cmp(&right.span.column))
        });

        Ok(LoweringArtifacts {
            lowered: LoweredModule {
                cell: self.cell.clone(),
                timing_aliases: self.timing_aliases.clone(),
                diagnostics: self.diagnostics.clone(),
            },
            signal_metadata: self.signal_metadata.clone(),
            assignment_provenance: self.assignment_provenance.clone(),
            timing_constraint_sources: self.timing_constraint_sources.clone(),
        })
    }

    fn collect_timing_aliases(&mut self) -> LowerResult<()> {
        for parameter in &self.module.parameters {
            if matches!(parameter.kind, ParamKind::Localparam | ParamKind::Specparam) {
                self.insert_timing_alias(&parameter.name, &parameter.span, Some(&parameter.value))?;
            }
        }
        for item in &self.module.items {
            match &item.kind {
                ItemKind::Decl(decl)
                    if matches!(decl.kind, DeclKind::Localparam | DeclKind::Specparam) =>
                {
                    for name in &decl.names {
                        self.insert_timing_alias(name, &decl.span, decl.value.as_ref())?;
                    }
                }
                ItemKind::Specify(specify) => {
                    for specify_item in &specify.items {
                        if let SpecifyItem::Specparam(param) = specify_item {
                            self.insert_timing_alias(&param.name, &param.span, Some(&param.value))?;
                        }
                    }
                }
                _ => {}
            }
        }

        // Resolve from the complete source map so forward references behave the
        // same as backward references. BTreeMap order also makes cycle errors
        // deterministic rather than dependent on source traversal order.
        let names = self
            .timing_alias_sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for name in names {
            let span = self.timing_alias_sources[&name].span.clone();
            self.resolve_timing_alias(&name, &span)?;
        }
        Ok(())
    }

    fn insert_timing_alias(
        &mut self,
        name: &str,
        span: &Span,
        value: Option<&SvExpr>,
    ) -> LowerResult<()> {
        let value = value.ok_or_else(|| {
            Diagnostic::new(
                span.clone(),
                format!("timing alias `{name}` must have a value"),
            )
        })?;
        if let Some(previous) = self.timing_alias_sources.get(name) {
            return Err(Diagnostic::new(
                span.clone(),
                format!(
                    "duplicate timing alias `{name}`; first declared at {}:{}:{}",
                    previous.span.path.display(),
                    previous.span.line,
                    previous.span.column
                ),
            ));
        }
        self.timing_alias_sources.insert(
            name.to_string(),
            TimingAliasSource {
                span: span.clone(),
                value: value.clone(),
            },
        );
        Ok(())
    }

    fn resolve_timing_alias(
        &mut self,
        name: &str,
        reference_span: &Span,
    ) -> LowerResult<TimingExpr> {
        if let Some(resolved) = self.timing_aliases.get(name) {
            return Ok(resolved.clone());
        }
        if let Some(position) = self
            .timing_alias_stack
            .iter()
            .position(|active| active == name)
        {
            let mut cycle = self.timing_alias_stack[position..].to_vec();
            cycle.push(name.to_string());
            return Err(Diagnostic::new(
                reference_span.clone(),
                format!("cyclic timing alias dependency: {}", cycle.join(" -> ")),
            ));
        }
        let source = self
            .timing_alias_sources
            .get(name)
            .cloned()
            .ok_or_else(|| {
                Diagnostic::new(
                    reference_span.clone(),
                    format!("unresolvable timing alias `{name}`"),
                )
            })?;
        self.timing_alias_stack.push(name.to_string());
        let lowered = self.lower_timing_expr(&source.value);
        self.timing_alias_stack.pop();
        let lowered = lowered?;
        self.timing_aliases
            .insert(name.to_string(), lowered.clone());
        Ok(lowered)
    }

    fn collect_specify_delays(&mut self) -> LowerResult<()> {
        for item in &self.module.items {
            let ItemKind::Specify(specify) = &item.kind else {
                continue;
            };
            for specify_item in &specify.items {
                let SpecifyItem::Path(path) = specify_item else {
                    continue;
                };
                let controls = path
                    .controls
                    .iter()
                    .map(|control| {
                        let signal = scalar_expr_symbol(control).ok_or_else(|| {
                            Diagnostic::new(
                                control.span.clone(),
                                "specify path control must be a scalar symbol",
                            )
                        })?;
                        TimingControlSource::new(signal, None, control.span.clone())
                    })
                    .collect::<LowerResult<Vec<_>>>()?;
                let target = scalar_expr_symbol(&path.target).ok_or_else(|| {
                    Diagnostic::new(
                        path.target.span.clone(),
                        "specify path target must be a scalar symbol",
                    )
                })?;
                let delay = self.lower_delay_tuple(&path.span, &path.delays)?;
                let constraint = TimingConstraintSource::new_with_target_span(
                    self.timing_constraint_sources.len(),
                    controls,
                    target.clone(),
                    delay.clone(),
                    path.target.span.clone(),
                    path.span.clone(),
                )?;
                self.timing_constraint_sources.push(constraint);
                self.specify_delays
                    .entry(target)
                    .or_default()
                    .push(SpecifyDelay {
                        path_span: path.span.clone(),
                        delay,
                    });
            }
        }
        Ok(())
    }

    fn source_delay_for(
        &mut self,
        target: &str,
        explicit: Option<&Delay>,
        source_origin: SourceAssignmentOrigin,
    ) -> LowerResult<SelectedDelay> {
        match explicit {
            Some(delay) => Ok(SelectedDelay {
                tuple: self.lower_delay_tuple(&delay.span, &delay.values)?,
                origin: if source_origin == SourceAssignmentOrigin::Primitive {
                    AssignmentDelayOrigin::PrimitiveSourceDelay
                } else {
                    AssignmentDelayOrigin::ExplicitSourceDelay
                },
            }),
            None => Ok(self.specify_delay_for(target)),
        }
    }

    fn specify_delay_for(&mut self, target: &str) -> SelectedDelay {
        if self.timing_policy == TimingLoweringPolicy::DecompositionBaseline {
            return SelectedDelay {
                tuple: zero_delay_tuple(),
                origin: AssignmentDelayOrigin::ImplicitZero,
            };
        }
        let Some(matches) = self.specify_delays.get(target) else {
            return SelectedDelay {
                tuple: zero_delay_tuple(),
                origin: AssignmentDelayOrigin::ImplicitZero,
            };
        };
        let first = matches[0].delay.clone();
        let additional_path_span = matches.get(1).map(|candidate| candidate.path_span.clone());
        if let Some(span) = additional_path_span
            && self
                .ignored_additional_specify_targets
                .insert(target.to_string())
        {
            self.diagnostics.push(Diagnostic::intentional_ignore(
                span,
                format!(
                    "additional control-dependent specify path for target `{target}` is intentionally ignored because delay-tuple lowering temporarily selects the first source-ordered path for the target"
                ),
            ));
        }
        SelectedDelay {
            tuple: first,
            origin: AssignmentDelayOrigin::LegacySelectedSpecifyFallback,
        }
    }

    fn lower_item(&mut self, item: &Item) -> LowerResult<()> {
        match &item.kind {
            ItemKind::Assign(assign) => {
                self.lower_continuous_assign(assign)?;
                Ok(())
            }
            ItemKind::Primitive(call) => self.lower_primitive_call(call),
            ItemKind::Initial(stmt) => self.lower_initial(stmt),
            ItemKind::AlwaysLatch(always) => {
                let condition = always
                    .condition
                    .as_ref()
                    .map(|expr| self.lower_expr(expr))
                    .transpose()?;
                self.lower_procedural_body(
                    &always.body,
                    ProceduralContext::stateful(condition, Vec::new()),
                )
            }
            ItemKind::Always(always) => {
                let stateful = matches!(always.kind, AlwaysKind::Ff)
                    || always
                        .sensitivity
                        .as_ref()
                        .map(|sensitivity| sensitivity_is_stateful(sensitivity, always.kind))
                        .unwrap_or(false);
                let context = if stateful {
                    ProceduralContext::stateful(
                        None,
                        always
                            .sensitivity
                            .as_ref()
                            .map(state_controls_from_sensitivity)
                            .unwrap_or_default(),
                    )
                } else {
                    ProceduralContext::combinational()
                };
                self.lower_procedural_body(&always.body, context)
            }
            ItemKind::Specify(_) | ItemKind::Decl(_) | ItemKind::Import(_) | ItemKind::Empty => {
                Ok(())
            }
            ItemKind::Instantiation(instantiation) if instantiation.module == "keeper" => {
                self.lower_keeper(instantiation)
            }
            ItemKind::ProcAssign(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Generate(_)
            | ItemKind::Block(_)
            | ItemKind::If(_) => Err(Diagnostic::new(
                item.span.clone(),
                "unsupported item for lowering",
            )),
        }
    }

    fn lower_keeper(&mut self, instantiation: &Instantiation) -> LowerResult<()> {
        let keeper = resolve_keeper_ast_instantiation(instantiation, &self.signal_spans)?;
        self.emit_assignment(
            keeper.connection.target,
            Expr::value(ValueOperator::Keeper, vec![]),
            SelectedDelay {
                tuple: zero_delay_tuple(),
                origin: AssignmentDelayOrigin::KeeperZero,
            },
            SourceEmission::new(
                &keeper.connection.span,
                &keeper.connection.span,
                SourceAssignmentOrigin::Keeper,
                Vec::new(),
            ),
        )
    }

    fn lower_initial(&mut self, stmt: &AssignStmt) -> LowerResult<()> {
        let target = expr_symbol(&stmt.target).ok_or_else(|| {
            Diagnostic::new(
                stmt.target.span.clone(),
                "initial assignment target must be a scalar local signal",
            )
        })?;
        if !self.signal_names.contains(&target) {
            return Err(Diagnostic::new(
                stmt.target.span.clone(),
                "initial assignment target must be a scalar local signal",
            ));
        }
        let initial = contracted_initial_literal(&stmt.value).ok_or_else(|| {
            Diagnostic::new(
                stmt.value.span.clone(),
                "initial assignment value must be a contracted literal (0, 1, '0, '1, 'x, or 'z)",
            )
        })?;
        if !self.initialized_registers.insert(target.clone()) {
            return Err(Diagnostic::new(
                stmt.target.span.clone(),
                format!(
                    "multiple initial assignments for register `{target}` cannot be represented by one register initial value"
                ),
            ));
        }
        let register = self
            .cell
            .registers
            .iter_mut()
            .find(|register| register.name == target)
            .ok_or_else(|| {
                Diagnostic::new(
                    stmt.target.span.clone(),
                    format!(
                        "internal lowering invariant violated: initialized signal `{target}` is not a modeled register"
                    ),
                )
            })?;
        register.initial = initial;
        Ok(())
    }

    fn lower_procedural_body(
        &mut self,
        item: &Item,
        context: ProceduralContext,
    ) -> LowerResult<()> {
        match &item.kind {
            ItemKind::ProcAssign(stmt) => self.lower_procedural_assign(stmt, &context),
            ItemKind::Block(block) | ItemKind::Generate(block) => {
                for child in &block.items {
                    self.lower_procedural_body(child, context.clone())?;
                }
                Ok(())
            }
            ItemKind::If(stmt) => {
                if context.mode == ProceduralMode::Combinational {
                    return Err(Diagnostic::new(
                        item.span.clone(),
                        "conditional combinational procedural lowering is unsupported because the condition cannot be discarded",
                    ));
                }
                if let Some(else_branch) = &stmt.else_branch {
                    return Err(Diagnostic::new(
                        else_branch.span.clone(),
                        "unsupported procedural else branch",
                    ));
                }
                let next_condition = match &context.condition {
                    Some(parent) => Expr::value(
                        ValueOperator::And,
                        vec![parent.clone(), self.lower_expr(&stmt.condition)?],
                    ),
                    None => self.lower_expr(&stmt.condition)?,
                };
                self.lower_procedural_body(
                    &stmt.then_branch,
                    ProceduralContext::stateful(
                        Some(next_condition),
                        context.state_controls.clone(),
                    ),
                )
            }
            ItemKind::Initial(_)
            | ItemKind::Assign(_)
            | ItemKind::Specify(_)
            | ItemKind::Decl(_)
            | ItemKind::Import(_)
            | ItemKind::Empty
            | ItemKind::AlwaysLatch(_)
            | ItemKind::Always(_)
            | ItemKind::Primitive(_)
            | ItemKind::Instantiation(_) => Err(Diagnostic::new(
                item.span.clone(),
                "unsupported procedural body for lowering",
            )),
        }
    }

    fn lower_procedural_assign(
        &mut self,
        stmt: &AssignStmt,
        context: &ProceduralContext,
    ) -> LowerResult<()> {
        let target = expr_symbol(&stmt.target).ok_or_else(|| {
            Diagnostic::new(
                stmt.target.span.clone(),
                "expected assignment target symbol",
            )
        })?;
        let mut expr = self.lower_expr(&stmt.value)?;
        if context.mode == ProceduralMode::Stateful
            && let Some(condition) = &context.condition
        {
            expr = Expr::value(
                ValueOperator::Mux,
                vec![condition.clone(), expr, Expr::atom(target.clone())],
            );
        }
        let origin = match context.mode {
            ProceduralMode::Combinational => SourceAssignmentOrigin::ProceduralCombinational,
            ProceduralMode::Stateful => SourceAssignmentOrigin::ProceduralStateful,
        };
        let delay = self.source_delay_for(&target, None, origin)?;
        self.emit_assignment(
            target,
            expr,
            delay,
            SourceEmission::new(
                &stmt.span,
                &stmt.value.span,
                origin,
                context.state_controls.clone(),
            ),
        )
    }

    fn lower_continuous_assign(&mut self, assign: &AssignDecl) -> LowerResult<()> {
        let target = expr_symbol(&assign.target).ok_or_else(|| {
            Diagnostic::new(
                assign.target.span.clone(),
                "expected assignment target symbol",
            )
        })?;
        let mut expr = self.lower_continuous_value(&assign.value)?;
        if let Some(strength) = &assign.strength {
            expr = apply_strength(expr, lower_strength_pair(strength)?);
        }
        let delay = self.source_delay_for(
            &target,
            assign.delay.as_ref(),
            SourceAssignmentOrigin::Continuous,
        )?;
        self.emit_assignment(
            target,
            expr,
            delay,
            SourceEmission::new(
                &assign.span,
                &assign.value.span,
                SourceAssignmentOrigin::Continuous,
                Vec::new(),
            ),
        )
    }

    fn lower_continuous_value(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Group(inner) => self.lower_continuous_value(inner),
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                if let Some(driver) = self.lower_tristate_ternary(
                    condition.as_ref(),
                    then_expr.as_ref(),
                    else_expr.as_ref(),
                )? {
                    Ok(driver)
                } else {
                    self.lower_expr(expr)
                }
            }
            _ => self.lower_expr(expr),
        }
    }

    fn lower_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Path(segments) => Ok(Expr::atom(segments.join("::"))),
            ExprKind::Integer(value) | ExprKind::Real(value) => Ok(Expr::atom(value.clone())),
            ExprKind::Constant(kind) => Ok(Expr::atom(match kind {
                ConstKind::Zero => "0",
                ConstKind::One => "1",
                ConstKind::Z => {
                    return Err(Diagnostic::new(
                        expr.span.clone(),
                        "high-Z is not a contracted ordinary driven value",
                    ));
                }
                ConstKind::X => "x",
            })),
            ExprKind::Group(inner) => self.lower_expr(inner),
            ExprKind::Unary { op, expr: operand } => match op {
                UnaryOp::Not | UnaryOp::BitNot => self.lower_not_expr(operand),
                UnaryOp::Plus | UnaryOp::Minus => Err(Diagnostic::new(
                    expr.span.clone(),
                    "unary arithmetic is not a contracted value expression",
                )),
            },
            ExprKind::Binary { op, left, right } => {
                let operator = match op {
                    BinaryOp::BitAnd | BinaryOp::LogicalAnd => ValueOperator::And,
                    BinaryOp::BitOr | BinaryOp::LogicalOr => ValueOperator::Or,
                    BinaryOp::BitXor => ValueOperator::Xor,
                    BinaryOp::BitNand => ValueOperator::Nand,
                    BinaryOp::BitNor => ValueOperator::Nor,
                    BinaryOp::BitXnor => ValueOperator::Xnor,
                    BinaryOp::Eq => ValueOperator::Eq,
                    BinaryOp::CaseEq => ValueOperator::CaseEq,
                    BinaryOp::Neq => ValueOperator::Neq,
                    BinaryOp::CaseNeq => ValueOperator::CaseNeq,
                    BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Less
                    | BinaryOp::Greater => {
                        return Err(Diagnostic::new(
                            expr.span.clone(),
                            "arithmetic and relational operators are not contracted value expressions",
                        ));
                    }
                };
                if matches!(op, BinaryOp::BitAnd | BinaryOp::LogicalAnd) {
                    let mut operands = Vec::new();
                    collect_and_operands(left, &mut operands);
                    collect_and_operands(right, &mut operands);
                    let mut items = Vec::with_capacity(operands.len() + 1);
                    for operand in operands {
                        items.push(self.lower_expr(operand)?);
                    }
                    return Ok(Expr::value(operator, items));
                }
                if matches!(op, BinaryOp::BitOr | BinaryOp::LogicalOr) {
                    let mut operands = Vec::new();
                    collect_or_operands(left, &mut operands);
                    collect_or_operands(right, &mut operands);
                    let mut items = Vec::with_capacity(operands.len() + 1);
                    for operand in operands {
                        items.push(self.lower_expr(operand)?);
                    }
                    return Ok(Expr::value(operator, items));
                }
                let operands = if matches!(
                    op,
                    BinaryOp::Eq | BinaryOp::CaseEq | BinaryOp::Neq | BinaryOp::CaseNeq
                ) {
                    vec![
                        self.lower_equality_operand(left)?,
                        self.lower_equality_operand(right)?,
                    ]
                } else {
                    vec![self.lower_expr(left)?, self.lower_expr(right)?]
                };
                Ok(Expr::value(operator, operands))
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                if self.is_z_expr(then_expr) || self.is_z_expr(else_expr) {
                    return Err(Diagnostic::new(
                        expr.span.clone(),
                        "high-Z ternary is legal only as the root value of a continuous driver",
                    ));
                }
                Ok(Expr::value(
                    ValueOperator::Mux,
                    vec![
                        self.lower_expr(condition)?,
                        self.lower_expr(then_expr)?,
                        self.lower_expr(else_expr)?,
                    ],
                ))
            }
            ExprKind::Call { .. } => Err(Diagnostic::new(
                expr.span.clone(),
                "function calls are not contracted value expressions",
            )),
        }
    }

    fn lower_equality_operand(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Constant(ConstKind::Z) => Ok(Expr::atom("z")),
            ExprKind::Group(inner) => self.lower_equality_operand(inner),
            _ => self.lower_expr(expr),
        }
    }

    fn lower_tristate_ternary(
        &mut self,
        condition: &SvExpr,
        then_expr: &SvExpr,
        else_expr: &SvExpr,
    ) -> LowerResult<Option<Expr>> {
        if self.is_z_expr(else_expr) {
            return Ok(Some(Expr::value(
                ValueOperator::BufIf1,
                vec![self.lower_expr(then_expr)?, self.lower_expr(condition)?],
            )));
        }
        if self.is_z_expr(then_expr) {
            return Ok(Some(Expr::value(
                ValueOperator::BufIf0,
                vec![self.lower_expr(else_expr)?, self.lower_expr(condition)?],
            )));
        }
        Ok(None)
    }

    fn is_z_expr(&self, expr: &SvExpr) -> bool {
        match &expr.kind {
            ExprKind::Constant(ConstKind::Z) => true,
            ExprKind::Group(inner) => self.is_z_expr(inner),
            _ => false,
        }
    }

    fn lower_primitive_call(&mut self, call: &PrimitiveCall) -> LowerResult<()> {
        match call.name.as_str() {
            "bufif0" | "bufif1" => self.lower_bufif_call(call),
            "nmos" | "pmos" | "rnmos" => self.lower_transistor_call(call),
            _ => Err(Diagnostic::new(
                call.span.clone(),
                "unsupported primitive for lowering",
            )),
        }
    }

    fn lower_transistor_call(&mut self, call: &PrimitiveCall) -> LowerResult<()> {
        if let Some(strength) = &call.strength {
            return Err(Diagnostic::new(
                strength.span.clone(),
                format!(
                    "strength-qualified {} is unsupported because direct transistor value operators do not carry source strength",
                    call.name
                ),
            ));
        }
        if call.args.len() != 3 {
            return Err(Diagnostic::new(
                call.span.clone(),
                format!("expected {} arity", call.name),
            ));
        }
        let drain = call.args[0].as_ref().ok_or_else(|| {
            Diagnostic::new(
                call.span.clone(),
                format!("expected {} drain argument", call.name),
            )
        })?;
        let source = call.args[1].as_ref().ok_or_else(|| {
            Diagnostic::new(
                call.span.clone(),
                format!("expected {} source argument", call.name),
            )
        })?;
        let gate = call.args[2].as_ref().ok_or_else(|| {
            Diagnostic::new(
                call.span.clone(),
                format!("expected {} gate argument", call.name),
            )
        })?;
        let drain = scalar_expr_symbol(drain).ok_or_else(|| {
            Diagnostic::new(
                drain.span.clone(),
                format!("expected {} drain scalar symbol", call.name),
            )
        })?;

        // Operand order is semantically significant: flatten source first,
        // then gate, before emitting the source-ordered transistor driver.
        let source = self.lower_expr(source)?;
        let gate = self.lower_expr(gate)?;
        let operator = match call.name.as_str() {
            "nmos" => ValueOperator::Nmos,
            "pmos" => ValueOperator::Pmos,
            "rnmos" => ValueOperator::Rnmos,
            _ => {
                return Err(Diagnostic::new(
                    call.span.clone(),
                    "uncontracted transistor value operator",
                ));
            }
        };
        let expr = Expr::value(operator, vec![source, gate]);
        let delay = self.source_delay_for(
            &drain,
            call.delay.as_ref(),
            SourceAssignmentOrigin::Primitive,
        )?;
        self.emit_assignment(
            drain,
            expr,
            delay,
            SourceEmission::new(
                &call.span,
                &call.span,
                SourceAssignmentOrigin::Primitive,
                Vec::new(),
            ),
        )
    }

    fn lower_bufif_call(&mut self, call: &PrimitiveCall) -> LowerResult<()> {
        if call.args.len() != 3 {
            return Err(Diagnostic::new(
                call.span.clone(),
                format!("expected {} arity", call.name),
            ));
        }
        let target = call.args[0]
            .as_ref()
            .ok_or_else(|| Diagnostic::new(call.span.clone(), "expected bufif target argument"))?;
        let value = call.args[1]
            .as_ref()
            .ok_or_else(|| Diagnostic::new(call.span.clone(), "expected bufif drive argument"))?;
        let control = call.args[2]
            .as_ref()
            .ok_or_else(|| Diagnostic::new(call.span.clone(), "expected bufif control argument"))?;
        let target = expr_symbol(target)
            .ok_or_else(|| Diagnostic::new(target.span.clone(), "expected bufif target symbol"))?;
        let mut operands = vec![self.lower_expr(value)?, self.lower_expr(control)?];
        let operator = match (call.name.as_str(), call.strength.as_ref()) {
            ("bufif0", Some(strength)) => {
                operands.extend(strength_operands(lower_strength_pair(strength)?));
                ValueOperator::BufIf0Strength
            }
            ("bufif1", Some(strength)) => {
                operands.extend(strength_operands(lower_strength_pair(strength)?));
                ValueOperator::BufIf1Strength
            }
            ("bufif0", None) => ValueOperator::BufIf0,
            ("bufif1", None) => ValueOperator::BufIf1,
            _ => {
                return Err(Diagnostic::new(
                    call.span.clone(),
                    "uncontracted bufif value operator",
                ));
            }
        };
        let expr = Expr::value(operator, operands);
        let delay = self.source_delay_for(
            &target,
            call.delay.as_ref(),
            SourceAssignmentOrigin::Primitive,
        )?;
        self.emit_assignment(
            target,
            expr,
            delay,
            SourceEmission::new(
                &call.span,
                &call.span,
                SourceAssignmentOrigin::Primitive,
                Vec::new(),
            ),
        )
    }

    fn emit_assignment(
        &mut self,
        target: String,
        expr: Expr,
        delay: SelectedDelay,
        emission: SourceEmission,
    ) -> LowerResult<()> {
        let source_assignment_order = self.next_source_assignment_order;
        self.next_source_assignment_order += 1;
        let expr = self.flatten_value_root(expr, &emission, source_assignment_order)?;
        self.push_validated_assignment(PendingAssignment {
            target,
            expr,
            delay: delay.tuple,
            source_assignment_order,
            diagnostic_span: emission.assignment_span.clone(),
            provenance_span: emission.assignment_span,
            origin: AssignmentOrigin::Source(emission.origin),
            delay_origin: delay.origin,
            state_controls: emission.state_controls,
        })
    }

    fn flatten_value_root(
        &mut self,
        expr: Expr,
        emission: &SourceEmission,
        source_assignment_order: usize,
    ) -> LowerResult<Expr> {
        let Expr::List(items) = expr else {
            return Ok(expr);
        };
        let mut items = items.into_iter();
        let head = items.next().ok_or_else(|| {
            Diagnostic::new(
                emission.assignment_span.clone(),
                "value operator list must not be empty",
            )
        })?;
        let Expr::Atom(head) = head else {
            return Err(Diagnostic::new(
                emission.assignment_span.clone(),
                "value operator must be an atom",
            ));
        };
        let operator = ValueOperator::parse(&head).ok_or_else(|| {
            Diagnostic::new(
                emission.assignment_span.clone(),
                format!("uncontracted value operator `{head}`"),
            )
        })?;
        let operands = items.collect::<Vec<_>>();
        if !operator.accepts_arity(operands.len()) {
            return Err(Diagnostic::new(
                emission.assignment_span.clone(),
                format!(
                    "wrong arity for value operator `{}`: got {}",
                    operator.as_str(),
                    operands.len()
                ),
            ));
        }

        let mut flat_operands = Vec::with_capacity(operands.len());
        for operand in operands {
            match operand {
                Expr::Atom(_) => flat_operands.push(operand),
                Expr::List(_) => {
                    let nested =
                        self.flatten_value_root(operand, emission, source_assignment_order)?;
                    let temporary = self.allocate_temporary();
                    self.push_validated_assignment(PendingAssignment {
                        target: temporary.clone(),
                        expr: nested,
                        delay: zero_delay_tuple(),
                        source_assignment_order,
                        diagnostic_span: emission.assignment_span.clone(),
                        provenance_span: emission.expression_span.clone(),
                        origin: AssignmentOrigin::GeneratedTemporary {
                            parent: emission.origin,
                        },
                        delay_origin: AssignmentDelayOrigin::GeneratedLogicalTemporaryZero,
                        state_controls: Vec::new(),
                    })?;
                    flat_operands.push(Expr::atom(temporary));
                }
            }
        }
        Ok(Expr::value(operator, flat_operands))
    }

    fn allocate_temporary(&mut self) -> String {
        loop {
            let name = format!("t{}", self.next_temp_index);
            self.next_temp_index += 1;
            if self.reserved_names.insert(name.clone()) {
                return name;
            }
        }
    }

    fn push_validated_assignment(&mut self, pending: PendingAssignment) -> LowerResult<()> {
        let assignment = Assignment {
            target: pending.target,
            expr: pending.expr,
            delay: pending.delay,
        };
        assignment.validate().map_err(|error| {
            Diagnostic::new(
                pending.diagnostic_span.clone(),
                format!("invalid lowered assignment: {error}"),
            )
        })?;
        let assignment_order = self.assignment_provenance.len();
        let provenance = AssignmentProvenance::new_with_delay_origin(
            assignment_order,
            pending.source_assignment_order,
            pending.provenance_span,
            pending.origin,
            pending.delay_origin,
            pending.state_controls,
        )?;
        self.cell.items.push(CellItem::Assignment(assignment));
        self.assignment_provenance.push(provenance);
        Ok(())
    }

    fn lower_not_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Group(inner) => self.lower_not_expr(inner),
            ExprKind::Binary {
                op: BinaryOp::BitAnd | BinaryOp::LogicalAnd,
                left,
                right,
            } => {
                let mut operands = Vec::new();
                collect_and_operands(left, &mut operands);
                collect_and_operands(right, &mut operands);
                let mut items = Vec::new();
                for operand in operands {
                    items.push(self.lower_expr(operand)?);
                }
                Ok(Expr::value(ValueOperator::Nand, items))
            }
            ExprKind::Binary {
                op: BinaryOp::BitOr | BinaryOp::LogicalOr,
                left,
                right,
            } => {
                let mut operands = Vec::new();
                collect_or_operands(left, &mut operands);
                collect_or_operands(right, &mut operands);
                let mut items = Vec::new();
                for operand in operands {
                    items.push(self.lower_expr(operand)?);
                }
                Ok(Expr::value(ValueOperator::Nor, items))
            }
            ExprKind::Binary {
                op: BinaryOp::BitXor,
                left,
                right,
            } => Ok(Expr::value(
                ValueOperator::Xnor,
                vec![self.lower_expr(left)?, self.lower_expr(right)?],
            )),
            _ => Ok(Expr::value(
                ValueOperator::Not,
                vec![self.lower_expr(expr)?],
            )),
        }
    }

    fn timing_atom(&self, value: impl Into<String>, source_span: &Span) -> LowerResult<TimingExpr> {
        TimingExpr::atom(value).map_err(|error| {
            Diagnostic::new(
                source_span.clone(),
                format!("invalid lowered timing expression: {error}"),
            )
        })
    }

    fn timing_operation(
        &self,
        operator: TimingOperator,
        operands: Vec<TimingExpr>,
        source_span: &Span,
    ) -> LowerResult<TimingExpr> {
        TimingExpr::operation(operator, operands).map_err(|error| {
            Diagnostic::new(
                source_span.clone(),
                format!("invalid lowered timing expression: {error}"),
            )
        })
    }

    fn lower_timing_expr(&mut self, expr: &SvExpr) -> LowerResult<TimingExpr> {
        match &expr.kind {
            ExprKind::Path(segments) => {
                if segments.len() == 1 {
                    let name = &segments[0];
                    if self.timing_alias_sources.contains_key(name) {
                        return self.resolve_timing_alias(name, &expr.span);
                    }
                }
                self.timing_atom(segments.join("::"), &expr.span)
            }
            ExprKind::Integer(value) | ExprKind::Real(value) => {
                self.timing_atom(value.clone(), &expr.span)
            }
            ExprKind::Constant(kind) => self.timing_atom(
                match kind {
                    ConstKind::Zero => "0",
                    ConstKind::One => "1",
                    ConstKind::Z => "z",
                    ConstKind::X => "x",
                },
                &expr.span,
            ),
            ExprKind::Group(inner) => self.lower_timing_expr(inner),
            ExprKind::Unary { op, expr: operand } => {
                let operator = match op {
                    UnaryOp::Plus => return self.lower_timing_expr(operand),
                    UnaryOp::Minus => TimingOperator::Subtract,
                    UnaryOp::Not | UnaryOp::BitNot => {
                        return Err(Diagnostic::new(
                            expr.span.clone(),
                            "Boolean operators are not part of the timing contract",
                        ));
                    }
                };
                let zero = self.timing_atom("0", &expr.span)?;
                let operand = self.lower_timing_expr(operand)?;
                self.timing_operation(operator, vec![zero, operand], &expr.span)
            }
            ExprKind::Binary { op, left, right } => {
                let operator = match op {
                    BinaryOp::Add => TimingOperator::Add,
                    BinaryOp::Sub => TimingOperator::Subtract,
                    BinaryOp::Mul => TimingOperator::Multiply,
                    BinaryOp::Div => TimingOperator::Divide,
                    BinaryOp::BitAnd
                    | BinaryOp::LogicalAnd
                    | BinaryOp::BitOr
                    | BinaryOp::LogicalOr
                    | BinaryOp::BitXor
                    | BinaryOp::BitNand
                    | BinaryOp::BitNor
                    | BinaryOp::BitXnor
                    | BinaryOp::Eq
                    | BinaryOp::CaseEq
                    | BinaryOp::Neq
                    | BinaryOp::CaseNeq => {
                        return Err(Diagnostic::new(
                            expr.span.clone(),
                            "operator is not part of the timing contract",
                        ));
                    }
                    BinaryOp::Greater => TimingOperator::Greater,
                    BinaryOp::Less => {
                        return Err(Diagnostic::new(
                            expr.span.clone(),
                            "less-than is not part of the timing contract",
                        ));
                    }
                };
                let left = self.lower_timing_expr(left)?;
                let right = self.lower_timing_expr(right)?;
                self.timing_operation(operator, vec![left, right], &expr.span)
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                let condition = self.lower_timing_expr(condition)?;
                let then_expr = self.lower_timing_expr(then_expr)?;
                let else_expr = self.lower_timing_expr(else_expr)?;
                self.timing_operation(
                    TimingOperator::Mux,
                    vec![condition, then_expr, else_expr],
                    &expr.span,
                )
            }
            ExprKind::Call { callee, args } => self.lower_timing_call(callee, args),
        }
    }

    fn lower_delay_tuple(
        &mut self,
        tuple_span: &Span,
        values: &[Option<SvExpr>],
    ) -> LowerResult<DelayTuple> {
        if !(1..=3).contains(&values.len()) {
            return Err(Diagnostic::new(
                tuple_span.clone(),
                format!(
                    "delay tuple must contain between one and three entries; got {}",
                    values.len()
                ),
            ));
        }

        let mut components = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let value = value.as_ref().ok_or_else(|| {
                Diagnostic::new(
                    tuple_span.clone(),
                    format!(
                        "explicitly omitted delay tuple entry {} is unsupported",
                        index + 1
                    ),
                )
            })?;
            components.push(self.lower_timing_expr(value)?);
        }

        let mut components = components.into_iter();
        let first = components.next().expect("validated nonempty tuple");
        Ok(match values.len() {
            1 => DelayTuple::One(first),
            2 => DelayTuple::Two {
                rise: first,
                fall: components.next().expect("validated two-entry tuple"),
            },
            3 => DelayTuple::Three {
                rise: first,
                fall: components.next().expect("validated three-entry tuple"),
                turn_off: components.next().expect("validated three-entry tuple"),
            },
            _ => unreachable!("validated delay tuple arity"),
        })
    }

    fn lower_timing_call(
        &mut self,
        callee: &SvExpr,
        args: &[Option<SvExpr>],
    ) -> LowerResult<TimingExpr> {
        let name = expr_symbol(callee).unwrap_or_else(|| render_call_callee(callee));
        match name.as_str() {
            "tpd_elmore" => {
                if args.len() != 2 {
                    return Err(Diagnostic::new(
                        callee.span.clone(),
                        "expected tpd_elmore arity",
                    ));
                }
                let wire = args[0].as_ref().ok_or_else(|| {
                    Diagnostic::new(callee.span.clone(), "expected wire argument")
                })?;
                let resistance = args[1].as_ref().ok_or_else(|| {
                    Diagnostic::new(callee.span.clone(), "expected resistance argument")
                })?;
                let wire = self.lower_timing_expr(wire)?;
                let wire = self.timing_operation(TimingOperator::Wire, vec![wire], &callee.span)?;
                let resistance = self.lower_timing_resistance(resistance)?;
                self.timing_operation(TimingOperator::Elmore, vec![wire, resistance], &callee.span)
            }
            "tpd_z" => {
                let Some(arg) = args.iter().find_map(|arg| arg.as_ref()) else {
                    return Err(Diagnostic::new(
                        callee.span.clone(),
                        "expected tpd_z argument",
                    ));
                };
                self.lower_timing_expr(arg)
            }
            "R_pmos_ohm" => self.lower_timing_resistance_call(TimingOperator::Pmos, callee, args),
            "R_nmos_ohm" => self.lower_timing_resistance_call(TimingOperator::Nmos, callee, args),
            _ => Err(Diagnostic::new(
                callee.span.clone(),
                format!("uncontracted timing function `{name}`"),
            )),
        }
    }

    fn lower_timing_resistance(&mut self, expr: &SvExpr) -> LowerResult<TimingExpr> {
        // Resistance networks use the ordinary recursive timing grammar. In
        // particular, do not peel off a resistance call from multiplication:
        // the outer factor is part of the modeled expression.
        self.lower_timing_expr(expr)
    }

    fn lower_timing_resistance_call(
        &mut self,
        operator: TimingOperator,
        callee: &SvExpr,
        args: &[Option<SvExpr>],
    ) -> LowerResult<TimingExpr> {
        if args.len() != 1 {
            return Err(Diagnostic::new(
                callee.span.clone(),
                "expected resistance function arity 1",
            ));
        }
        let Some(arg) = args.first().and_then(|arg| arg.as_ref()) else {
            return Err(Diagnostic::new(
                callee.span.clone(),
                "expected resistance argument",
            ));
        };
        let value = self.extract_unit_factor(arg)?;
        debug_assert!(matches!(
            operator,
            TimingOperator::Pmos | TimingOperator::Nmos
        ));
        self.timing_operation(operator, vec![value], &callee.span)
    }

    fn extract_unit_factor(&mut self, expr: &SvExpr) -> LowerResult<TimingExpr> {
        match &expr.kind {
            ExprKind::Group(inner) => self.extract_unit_factor(inner),
            ExprKind::Binary {
                op: BinaryOp::Mul,
                left,
                right,
            } if is_l_unit(left) => self.lower_resistance_factor(right),
            ExprKind::Binary {
                op: BinaryOp::Mul,
                left,
                right,
            } if is_l_unit(right) => self.lower_resistance_factor(left),
            ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Path(_) => {
                self.lower_resistance_factor(expr)
            }
            _ => Err(Diagnostic::new(
                expr.span.clone(),
                "unsupported timing factor",
            )),
        }
    }

    fn lower_resistance_factor(&mut self, expr: &SvExpr) -> LowerResult<TimingExpr> {
        match &expr.kind {
            ExprKind::Group(inner) => self.lower_resistance_factor(inner),
            ExprKind::Path(segments) if segments.len() == 1 && segments[0] == "L_unit" => {
                self.timing_atom("1", &expr.span)
            }
            ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Path(_) => {
                self.lower_timing_expr(expr)
            }
            _ => Err(Diagnostic::new(
                expr.span.clone(),
                "resistance factor must be an integer, real, or scalar timing atom",
            )),
        }
    }
}

fn timing_signal_metadata(
    analysis: &crate::analyze::ModuleAnalysis,
) -> LowerResult<Vec<TimingSignalMetadata>> {
    let modeled_registers = analysis.registers.iter().collect::<BTreeSet<_>>();
    analysis
        .symbols
        .iter()
        .filter(|(_, symbol)| {
            matches!(
                symbol.category,
                crate::analyze::SymbolCategory::Port | crate::analyze::SymbolCategory::Declaration
            )
        })
        .map(|(name, symbol)| {
            let mut roles = BTreeSet::new();
            match symbol.category {
                crate::analyze::SymbolCategory::Port => {
                    let port = &analysis.ports[name];
                    if port.is_input {
                        roles.insert(TimingSignalRole::Input);
                    }
                    if port.is_output {
                        roles.insert(TimingSignalRole::Output);
                    }
                    if port.direction == Direction::Inout {
                        roles.insert(TimingSignalRole::Inout);
                    }
                    if port.direction == Direction::Inout
                        || port
                            .modifiers
                            .iter()
                            .any(|modifier| matches!(modifier.as_str(), "tri" | "wire"))
                    {
                        roles.insert(TimingSignalRole::ResolvedNet);
                    }
                }
                crate::analyze::SymbolCategory::Declaration => {
                    roles.insert(TimingSignalRole::Internal);
                    if matches!(
                        analysis.declarations[name].kind,
                        DeclKind::Tri | DeclKind::Wire
                    ) {
                        roles.insert(TimingSignalRole::ResolvedNet);
                    }
                }
                crate::analyze::SymbolCategory::Parameter
                | crate::analyze::SymbolCategory::Localparam
                | crate::analyze::SymbolCategory::Specparam => {
                    unreachable!("timing signal metadata filters non-signal symbol categories")
                }
            }
            if modeled_registers.contains(&name) {
                roles.insert(TimingSignalRole::ModeledRegister);
            }
            TimingSignalMetadata::new(name.clone(), roles, symbol.span.clone())
        })
        .collect()
}

fn state_controls_from_sensitivity(sensitivity: &Sensitivity) -> Vec<StateControlProvenance> {
    let SensitivityKind::List(events) = &sensitivity.kind else {
        return Vec::new();
    };
    events
        .iter()
        .map(|event| {
            let transition = match event.edge.as_deref() {
                Some("posedge") => Some(Transition::Rise),
                Some("negedge") => Some(Transition::Fall),
                Some(_) | None => None,
            };
            match event.expr.as_ref().and_then(scalar_expr_symbol) {
                Some(signal) => StateControlProvenance::new(signal, transition, event.span.clone()),
                None => StateControlProvenance::unrepresentable(transition, event.span.clone()),
            }
        })
        .collect()
}

fn zero_delay_tuple() -> DelayTuple {
    DelayTuple::One(TimingExpr::atom("0").expect("zero is a valid timing atom"))
}

fn lower_strength_pair(strength: &Strength) -> LowerResult<StrengthPair> {
    if strength.values.len() != 2 {
        return Err(Diagnostic::new(
            strength.span.clone(),
            format!(
                "drive strength must contain exactly two values; got {}: `{}`",
                strength.values.len(),
                render_strength_values(&strength.values)
            ),
        ));
    }
    StrengthPair::parse(&strength.values[0], &strength.values[1]).ok_or_else(|| {
        Diagnostic::new(
            strength.span.clone(),
            format!(
                "unsupported drive strength pair `{}`",
                render_strength_values(&strength.values)
            ),
        )
    })
}

fn render_strength_values(values: &[String]) -> String {
    format!("({})", values.join(", "))
}

fn strength_operands(pair: StrengthPair) -> [Expr; 2] {
    let (first, second) = pair.atoms();
    [Expr::atom(first), Expr::atom(second)]
}

fn apply_strength(expr: Expr, pair: StrengthPair) -> Expr {
    let operator = match &expr {
        Expr::List(items) => match items.first() {
            Some(Expr::Atom(head)) if head == ValueOperator::BufIf0.as_str() => {
                Some(ValueOperator::BufIf0Strength)
            }
            Some(Expr::Atom(head)) if head == ValueOperator::BufIf1.as_str() => {
                Some(ValueOperator::BufIf1Strength)
            }
            _ => None,
        },
        Expr::Atom(_) => None,
    };
    if let Some(operator) = operator {
        let Expr::List(mut items) = expr else {
            unreachable!()
        };
        items.remove(0);
        items.extend(strength_operands(pair));
        Expr::value(operator, items)
    } else {
        let mut operands = vec![expr];
        operands.extend(strength_operands(pair));
        Expr::value(ValueOperator::DriveStrength, operands)
    }
}

fn expr_symbol(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) => Some(segments.join("::")),
        ExprKind::Group(inner) => expr_symbol(inner),
        _ => None,
    }
}

fn scalar_expr_symbol(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) if segments.len() == 1 => Some(segments[0].clone()),
        ExprKind::Group(inner) => scalar_expr_symbol(inner),
        _ => None,
    }
}

fn contracted_initial_literal(expr: &SvExpr) -> Option<LogicValue> {
    match &expr.kind {
        ExprKind::Constant(ConstKind::Zero) => Some(LogicValue::Zero),
        ExprKind::Constant(ConstKind::One) => Some(LogicValue::One),
        ExprKind::Constant(ConstKind::X) => Some(LogicValue::X),
        ExprKind::Constant(ConstKind::Z) => Some(LogicValue::Z),
        ExprKind::Integer(value) if value == "0" => Some(LogicValue::Zero),
        ExprKind::Integer(value) if value == "1" => Some(LogicValue::One),
        ExprKind::Group(inner) => contracted_initial_literal(inner),
        _ => None,
    }
}

fn render_call_callee(expr: &SvExpr) -> String {
    expr_symbol(expr).unwrap_or_else(|| "call".to_string())
}

fn collect_and_operands<'a>(expr: &'a SvExpr, out: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary {
            op: BinaryOp::BitAnd | BinaryOp::LogicalAnd,
            left,
            right,
        } => {
            collect_and_operands(left, out);
            collect_and_operands(right, out);
        }
        _ => out.push(expr),
    }
}

fn collect_or_operands<'a>(expr: &'a SvExpr, out: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary {
            op: BinaryOp::BitOr | BinaryOp::LogicalOr,
            left,
            right,
        } => {
            collect_or_operands(left, out);
            collect_or_operands(right, out);
        }
        _ => out.push(expr),
    }
}

fn is_l_unit(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Path(segments) => segments.len() == 1 && segments[0] == "L_unit",
        ExprKind::Group(inner) => is_l_unit(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticPolicy;
    use crate::serialize::{render_delay_tuple, render_expr, render_timing_expr};
    use std::fs;

    fn lower_path(path: &str) -> LoweredModule {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let input = fs::read_to_string(&path).unwrap();
        lower_file(&path, &input).unwrap()
    }

    fn assignment_strings(lowered: &LoweredModule) -> Vec<(String, String, String)> {
        lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some((
                    assignment.target.clone(),
                    render_expr(&assignment.expr),
                    render_timing_expr(assignment.delay.first()),
                )),
                _ => None,
            })
            .collect()
    }

    fn assignment_tuple_strings(lowered: &LoweredModule) -> Vec<(String, String, String)> {
        lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some((
                    assignment.target.clone(),
                    render_expr(&assignment.expr),
                    render_delay_tuple(&assignment.delay),
                )),
                _ => None,
            })
            .collect()
    }

    fn rendered_exprs(path: &str) -> Vec<String> {
        assignment_strings(&lower_path(path))
            .into_iter()
            .map(|(_, expr, _)| expr)
            .collect()
    }

    fn lower_snippet(input: &str) -> LowerResult<LoweredModule> {
        lower_file(Path::new("snippet.sv"), input)
    }

    fn lower_timing_snippet(input: &str) -> LowerResult<LoweredTimingModel> {
        let design = crate::parser::parse_file(Path::new("snippet.sv"), input)?;
        lower_design_with_timing_and_generate_mode(&design, GenerateMode::default())
    }

    fn lower_decomposed_timing_snippet(input: &str) -> LowerResult<LoweredDecomposedTimingModel> {
        let design = crate::parser::parse_file(Path::new("snippet.sv"), input)?;
        lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::default())
    }

    fn registers(lowered: &LoweredModule) -> Vec<(&str, LogicValue)> {
        lowered
            .cell
            .registers
            .iter()
            .map(|register| (register.name.as_str(), register.initial))
            .collect()
    }

    #[test]
    fn literal_initial_is_register_metadata_and_emits_no_assignment_or_diagnostic() {
        let lowered =
            lower_snippet("module sample(output logic q);\n  initial q = ('0);\nendmodule\n")
                .unwrap();
        assert_eq!(registers(&lowered), vec![("q", LogicValue::Zero)]);
        assert!(assignment_strings(&lowered).is_empty());
        assert!(lowered.diagnostics.is_empty());
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn initial_literals_normalize_to_typed_four_state_values() {
        let lowered = lower_snippet(
            "module sample(output logic i0, i1, u0, u1, ux, uz);\n\
             initial i0 = 0;\n\
             initial i1 = (((1)));\n\
             initial u0 = '0;\n\
             initial u1 = ('1);\n\
             initial ux = ((('x)));\n\
             initial uz = 'z;\n\
             endmodule\n",
        )
        .unwrap();
        assert_eq!(
            registers(&lowered),
            vec![
                ("i0", LogicValue::Zero),
                ("i1", LogicValue::One),
                ("u0", LogicValue::Zero),
                ("u1", LogicValue::One),
                ("ux", LogicValue::X),
                ("uz", LogicValue::Z),
            ]
        );
        assert!(assignment_strings(&lowered).is_empty());
        assert!(lowered.diagnostics.is_empty());
    }

    #[test]
    fn procedural_state_without_an_initializer_defaults_to_unknown() {
        let lowered = lower_snippet(
            "module sample(input logic clk, d, output logic q);\n  always_ff @(posedge clk) q <= d;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(registers(&lowered), vec![("q", LogicValue::X)]);
    }

    #[test]
    fn generate_selection_keeps_only_the_selected_initializer() {
        let source = r#"module sample(output logic q);
  generate
    if (nodelay) begin
      initial q = '1;
    end else begin
      initial q = '0;
    end
  endgenerate
endmodule
"#;
        let delayful =
            lower_file_with_generate_mode(Path::new("snippet.sv"), source, GenerateMode::Delayful)
                .unwrap();
        let nodelay =
            lower_file_with_generate_mode(Path::new("snippet.sv"), source, GenerateMode::Nodelay)
                .unwrap();

        assert_eq!(registers(&delayful), vec![("q", LogicValue::Zero)]);
        assert_eq!(registers(&nodelay), vec![("q", LogicValue::One)]);
        assert!(delayful.diagnostics.is_empty());
        assert!(nodelay.diagnostics.is_empty());
    }

    #[test]
    fn duplicate_initializers_fail_at_the_second_target_in_analysis_and_lowering() {
        let source =
            "module sample(output logic q);\n  initial q = '0;\n  initial (q) = '1;\nendmodule\n";
        let design = crate::parser::parse_file(Path::new("snippet.sv"), source).unwrap();
        let analysis = crate::analyze::analyze_design_structural(&design);
        let requirement = analysis
            .requirements
            .iter()
            .find(|requirement| requirement.capability_id == "invalid.initial.duplicate")
            .unwrap();
        assert_eq!(requirement.span, Span::new("snippet.sv", 3, 11));
        assert_eq!(
            analysis.disposition,
            crate::analyze::AnalysisDisposition::Failed
        );
        assert_eq!(
            requirement.reason,
            "multiple initial assignments for register `q` cannot be represented by one register initial value"
        );

        let mut lowerer = Lowerer::new(
            design.first_module().unwrap(),
            &analysis.modules[0],
            TimingLoweringPolicy::LegacyFirst,
        )
        .unwrap();
        let error = lowerer.lower_module().unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 3, 11));
        assert_eq!(error.message, requirement.reason);
    }

    #[test]
    fn invalid_initial_forms_fail_at_their_specific_expression_spans() {
        let nonliteral = lower_snippet(
            "module sample(input logic d, output logic q);\n  initial q = d;\nendmodule\n",
        )
        .unwrap_err();
        assert_eq!(nonliteral.span, Span::new("snippet.sv", 2, 15));
        assert_eq!(
            nonliteral.message,
            "initial assignment value must be a contracted literal (0, 1, '0, '1, 'x, or 'z)"
        );

        let integer_two =
            lower_snippet("module sample(output logic q);\n  initial q = 2;\nendmodule\n")
                .unwrap_err();
        assert_eq!(integer_two.span, Span::new("snippet.sv", 2, 15));
        assert_eq!(integer_two.message, nonliteral.message);

        let nonscalar = lower_snippet(
            "module sample(input logic d, output logic q);\n  initial q & d = '0;\nendmodule\n",
        )
        .unwrap_err();
        assert_eq!(nonscalar.span, Span::new("snippet.sv", 2, 11));
        assert_eq!(
            nonscalar.message,
            "initial assignment target must be a scalar local signal"
        );
    }

    #[test]
    fn lowers_keeper_as_distinct_zero_delay_source_ordered_driver() {
        let lowered = lower_snippet(
            "module sample(input logic a, en, output logic y);\n  assign y = a;\n  bufif0 (y, a, en);\n  keeper held(y);\n  assign y = en;\nendmodule\n",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("y".into(), "a".into(), "0".into()),
                ("y".into(), "(bufif0 a en)".into(), "0".into()),
                ("y".into(), "(keeper)".into(), "0".into()),
                ("y".into(), "en".into(), "0".into()),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn keeper_never_inherits_a_specify_delay() {
        let lowered = lower_snippet(
            "module sample(output logic y);\n  keeper held(y);\n  specify\n    (y *> y) = (9);\n  endspecify\nendmodule\n",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![("y".into(), "(keeper)".into(), "0".into())]
        );
    }

    #[test]
    fn malformed_keeper_lowering_reuses_typed_resolution_diagnostics() {
        let cases = [
            (
                "module bad(output logic y);\n  keeper #(1) hold(y);\nendmodule\n",
                Span::new("snippet.sv", 2, 12),
                "keeper instance `hold` does not accept parameter overrides",
            ),
            (
                "module bad(output logic y);\n  keeper hold(.target(y));\nendmodule\n",
                Span::new("snippet.sv", 2, 15),
                "keeper instance `hold` requires a positional connection",
            ),
            (
                "module bad(output logic y);\n  keeper hold();\nendmodule\n",
                Span::new("snippet.sv", 2, 3),
                "keeper instance `hold` requires exactly one positional connection",
            ),
            (
                "module bad(input logic a, output logic y);\n  keeper hold(y, a);\nendmodule\n",
                Span::new("snippet.sv", 2, 18),
                "keeper instance `hold` requires exactly one positional connection",
            ),
            (
                "module bad(input logic a, output logic y);\n  keeper hold(a & y);\nendmodule\n",
                Span::new("snippet.sv", 2, 15),
                "keeper instance `hold` target must be a scalar signal name",
            ),
            (
                "module bad(output logic y);\n  keeper hold(missing);\nendmodule\n",
                Span::new("snippet.sv", 2, 15),
                "unknown keeper target `missing` for instance `hold`",
            ),
        ];
        for (source, span, message) in cases {
            let error = lower_snippet(source).unwrap_err();
            assert_eq!(error.span, span);
            assert_eq!(error.message, message);
        }
    }

    #[test]
    fn blocking_and_nonblocking_latches_normalize_identically() {
        let blocking = lower_snippet(
            "module sample(input logic ena, d, output logic q);\n  always_latch if (ena) q = d;\nendmodule\n",
        )
        .unwrap();
        let nonblocking = lower_snippet(
            "module sample(input logic ena, d, output logic q);\n  always_latch if (ena) q <= d;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(registers(&blocking), vec![("q", LogicValue::X)]);
        assert_eq!(registers(&nonblocking), vec![("q", LogicValue::X)]);
        assert_eq!(
            assignment_strings(&blocking),
            assignment_strings(&nonblocking)
        );
        assert_eq!(
            assignment_strings(&blocking),
            vec![(
                "q".to_string(),
                "(mux ena d q)".to_string(),
                "0".to_string(),
            )]
        );
        blocking.cell.validate().unwrap();
        nonblocking.cell.validate().unwrap();
    }

    #[test]
    fn nested_state_conditions_and_data_are_flattened_dependency_first() {
        let lowered = lower_snippet(
            "module sample(input logic clk, ena, reset_n, d, r, output logic q);\n  always_ff @(posedge clk) if (ena) if (!reset_n) q <= d & r;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(registers(&lowered), vec![("q", LogicValue::X)]);
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "t0".to_string(),
                    "(not reset_n)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t1".to_string(),
                    "(and ena t0)".to_string(),
                    "0".to_string(),
                ),
                ("t2".to_string(), "(and d r)".to_string(), "0".to_string(),),
                (
                    "q".to_string(),
                    "(mux t1 t2 q)".to_string(),
                    "0".to_string(),
                ),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn later_stateful_assignments_remain_separate_in_source_priority_order() {
        let lowered = lower_snippet(
            "module sample(input logic clk, reset, set, d, output logic q);\n  always_ff @(posedge clk) begin\n    if (reset) q <= 0;\n    if (set) q = 1;\n    q <= d;\n  end\nendmodule\n",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "q".to_string(),
                    "(mux reset 0 q)".to_string(),
                    "0".to_string(),
                ),
                (
                    "q".to_string(),
                    "(mux set 1 q)".to_string(),
                    "0".to_string(),
                ),
                ("q".to_string(), "d".to_string(), "0".to_string()),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn unconditional_combinational_procedure_is_not_state_and_conditional_is_rejected() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, output logic y);\n  always_comb y = a & b;\nendmodule\n",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![("y".to_string(), "(and a b)".to_string(), "0".to_string(),)]
        );
        lowered.cell.validate().unwrap();

        let error = lower_snippet(
            "module sample(input logic ena, d, output logic y);\n  always_comb if (ena) y = d;\nendmodule\n",
        )
        .unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 2, 15));
        assert_eq!(
            error.message,
            "conditional combinational procedural lowering is unsupported because the condition cannot be discarded"
        );
    }

    #[test]
    fn compound_values_emit_dependencies_before_the_source_target() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, c, output logic y); assign y = !(a & (b | c)); endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t0".to_string(), "(or b c)".to_string(), "0".to_string()),
                ("y".to_string(), "(nand a t0)".to_string(), "0".to_string(),),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn temporary_sequence_is_module_global_and_preserves_source_order() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, c, d, output logic y, z);\
             assign y = a & (b | c);\
             assign z = !(d ^ (a & c));\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t0".to_string(), "(or b c)".to_string(), "0".to_string()),
                ("y".to_string(), "(and a t0)".to_string(), "0".to_string(),),
                ("t1".to_string(), "(and a c)".to_string(), "0".to_string(),),
                ("z".to_string(), "(xnor d t1)".to_string(), "0".to_string(),),
            ]
        );
    }

    #[test]
    fn temporary_names_skip_source_visible_symbols() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, c, output logic t0, y); assign y = a & (b | c); endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t1".to_string(), "(or b c)".to_string(), "0".to_string()),
                ("y".to_string(), "(and a t1)".to_string(), "0".to_string(),),
            ]
        );
    }

    #[test]
    fn only_the_source_target_keeps_a_modeled_delay() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, c, output logic y); assign #(7) y = a & (b | c); endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t0".to_string(), "(or b c)".to_string(), "0".to_string()),
                ("y".to_string(), "(and a t0)".to_string(), "7".to_string(),),
            ]
        );
    }

    #[test]
    fn lowers_and_gate_cells() {
        assert!(
            rendered_exprs("../sv-cells/sm83/cells/and3.sv")
                .contains(&"(and in1 in2 in3)".to_string())
        );
        assert!(
            rendered_exprs("../sv-cells/dmg_cpu_b/cells/and2.sv")
                .contains(&"(and in1 in2)".to_string())
        );
    }

    #[test]
    fn lowers_or_and_nor_cells() {
        assert!(
            rendered_exprs("../sv-cells/sm83/cells/or3_b.sv")
                .contains(&"(or in1 in2 in3)".to_string())
        );
        assert!(
            rendered_exprs("../sv-cells/sm83/cells/nor8_alu.sv")
                .contains(&"(nor in1 in2 in3 in4 in5 in6 in7 in8)".to_string())
        );
    }

    #[test]
    fn lowers_xor_and_xnor_cells() {
        assert!(
            rendered_exprs("../sv-cells/sm83/cells/xor_idu_l.sv")
                .contains(&"(xor in1 in2)".to_string())
        );
        assert!(
            rendered_exprs("../sv-cells/dmg_cpu_b/cells/xor.sv")
                .contains(&"(xor in1 in2)".to_string())
        );
        assert!(
            rendered_exprs("../sv-cells/dmg_cpu_b/cells/xnor.sv")
                .contains(&"(xnor in1 in2)".to_string())
        );
    }

    #[test]
    fn lowers_register_latch_family_with_normalized_assignments() {
        let lowered = lower_path("../sv-cells/sm83/cells/dffr_cc_ee_reg_ie_bit.sv");
        assert_eq!(
            registers(&lowered),
            vec![
                ("ff1", LogicValue::Zero),
                ("ff2", LogicValue::Zero),
                ("q_n", LogicValue::Zero),
            ]
        );
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "t0".to_string(),
                    "(and d clk_n ena)".to_string(),
                    "0".to_string(),
                ),
                ("t1".to_string(), "(not d)".to_string(), "0".to_string()),
                ("t2".to_string(), "(not clk)".to_string(), "0".to_string()),
                ("t3".to_string(), "(not ena_n)".to_string(), "0".to_string(),),
                (
                    "t4".to_string(),
                    "(and t1 t2 t3)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t5".to_string(),
                    "(or t0 t4 r)".to_string(),
                    "0".to_string(),
                ),
                ("t6".to_string(), "(not r)".to_string(), "0".to_string()),
                ("t7".to_string(), "(and d t6)".to_string(), "0".to_string(),),
                (
                    "ff1".to_string(),
                    "(mux t5 t7 ff1)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t8".to_string(),
                    "(and ff1 clk)".to_string(),
                    "0".to_string(),
                ),
                ("t9".to_string(), "(not ff1)".to_string(), "0".to_string()),
                (
                    "t10".to_string(),
                    "(not clk_n)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t11".to_string(),
                    "(and t9 t10)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t12".to_string(),
                    "(or t8 t11)".to_string(),
                    "0".to_string(),
                ),
                ("t13".to_string(), "(not ff1)".to_string(), "0".to_string(),),
                (
                    "ff2".to_string(),
                    "(mux t12 t13 ff2)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t14".to_string(),
                    "(and ff2 clk)".to_string(),
                    "0".to_string(),
                ),
                ("t15".to_string(), "(not ff2)".to_string(), "0".to_string(),),
                (
                    "t16".to_string(),
                    "(not clk_n)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t17".to_string(),
                    "(and t15 t16)".to_string(),
                    "0".to_string(),
                ),
                (
                    "t18".to_string(),
                    "(or t14 t17)".to_string(),
                    "0".to_string(),
                ),
                (
                    "q_n".to_string(),
                    "(mux t18 ff2 q_n)".to_string(),
                    "(+ (+ (elmore (wire 55) (* (pmos 3) 2)) (elmore (wire 23) (* (nmos 3) 2))) (elmore (wire L_q_n) (pmos 13)))".to_string(),
                ),
                (
                    "q".to_string(),
                    "(not q_n)".to_string(),
                    "(+ (+ (+ (elmore (wire 55) (* (nmos 3) 2)) (elmore (wire 23) (* (pmos 3) 2))) (elmore (wire L_q_n) (nmos 6))) (elmore (wire L_q) (pmos 13)))".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn lowers_block_wrapped_latch_body() {
        let lowered = lower_path("../sv-cells/dmg_cpu_b/cells/nand_latch.sv");
        assert_eq!(
            registers(&lowered),
            vec![("q", LogicValue::X), ("q_n", LogicValue::X)]
        );
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t0".to_string(), "(not s_n)".to_string(), "0".to_string()),
                ("t1".to_string(), "(not r_n)".to_string(), "0".to_string()),
                ("t2".to_string(), "(or t0 t1)".to_string(), "0".to_string(),),
                ("t3".to_string(), "(not s_n)".to_string(), "0".to_string()),
                (
                    "q".to_string(),
                    "(mux t2 t3 q)".to_string(),
                    "(elmore (wire L_q) (pmos 35))".to_string(),
                ),
                ("t4".to_string(), "(not s_n)".to_string(), "0".to_string()),
                ("t5".to_string(), "(not r_n)".to_string(), "0".to_string()),
                ("t6".to_string(), "(or t4 t5)".to_string(), "0".to_string(),),
                ("t7".to_string(), "(not r_n)".to_string(), "0".to_string()),
                (
                    "q_n".to_string(),
                    "(mux t6 t7 q_n)".to_string(),
                    "(+ (elmore (wire L_q) (nmos 35)) (elmore (wire L_q_n) (pmos 35)))".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn lowers_simple_latch_and_continuous_output() {
        let lowered = lower_path("../sv-cells/dmg_cpu_b/cells/dlatch.sv");
        assert_eq!(registers(&lowered), vec![("q", LogicValue::Zero)]);
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "q".to_string(),
                    "(mux ena d q)".to_string(),
                    "(+ (+ (elmore (wire 73) (pmos 10)) (elmore (wire 101) (nmos 10))) (elmore (wire L_q) (pmos 35)))".to_string(),
                ),
                (
                    "q_n".to_string(),
                    "(not q)".to_string(),
                    "(+ (+ (+ (elmore (wire 73) (nmos 10)) (elmore (wire 101) (pmos 10))) (elmore (wire 127) (nmos 10))) (elmore (wire L_q_n) (pmos 35)))".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn lowers_tri_state_assign_and_precharge_cell() {
        let lowered = lower_path("../sv-cells/sm83/cells/not_pch_x2_alu.sv");
        assert_eq!(
            assignment_strings(&lowered)
                .into_iter()
                .map(|(target, expr, _)| (target, expr))
                .collect::<Vec<_>>(),
            vec![
                ("y".to_string(), "(not in)".to_string()),
                (
                    "in".to_string(),
                    "(bufif0-strength 1 pch_n strong1 highz0)".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn lowers_direct_bufif_precharge_and_tristate_variants() {
        let lowered = lower_path("../sv-cells/dmg_cpu_b/cells/pad_bidir.sv");
        assert_eq!(
            assignment_strings(&lowered)
                .into_iter()
                .map(|(target, expr, _)| (target, expr))
                .collect::<Vec<_>>(),
            vec![
                (
                    "pad".to_string(),
                    "(bufif1-strength 0 ndrv highz1 strong0)".to_string(),
                ),
                (
                    "pad".to_string(),
                    "(bufif0-strength 1 pdrv_n strong1 highz0)".to_string(),
                ),
                ("i_n".to_string(), "(not pad)".to_string()),
            ]
        );
    }

    #[test]
    fn lowers_tristate_assigns_with_repeated_drivers_in_source_order() {
        let lowered = lower_path("../sv-cells/sm83/cells/reg_pc_out_bit012.sv");
        let assignments = assignment_strings(&lowered);
        let y1_index = assignments
            .iter()
            .position(|(target, _, _)| target == "y1")
            .unwrap();
        assert_eq!(
            assignments[y1_index - 3..=y1_index]
                .iter()
                .map(|(target, expr, _)| (target.as_str(), expr.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("t0", "(and in1 in2)"),
                ("t1", "(and in3 in4)"),
                ("t2", "(or t0 t1)"),
                ("y1", "(bufif1-strength 0 t2 highz1 strong0)"),
            ]
        );
        let y4_assignments = assignments
            .iter()
            .filter(|(target, _, _)| target == "y4")
            .map(|(_, expr, _)| expr.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            y4_assignments,
            vec![
                "(bufif1-strength 0 t7 highz1 strong0)".to_string(),
                "(bufif1-strength 0 in9 highz1 strong0)".to_string(),
            ]
        );
    }

    #[test]
    fn delay_tuples_preserve_exact_arity_and_first_projection() {
        for (delay, expected) in [
            ("#(1)", "(delay 1)"),
            ("#(1, 2)", "(delay 1 2)"),
            ("#(1, 2, 3)", "(delay 1 2 3)"),
        ] {
            let input = format!(
                "module sample(input logic a, output logic y); assign {delay} y = a; endmodule"
            );
            let lowered = lower_snippet(&input).unwrap();
            assert_eq!(assignment_tuple_strings(&lowered)[0].2, expected);
            assert_eq!(assignment_strings(&lowered)[0].2, "1");
            assert!(lowered.diagnostics.is_empty());
        }
        let lowered =
            lower_snippet("module sample(input logic a, output logic y); assign y = a; endmodule")
                .unwrap();
        assert_eq!(assignment_tuple_strings(&lowered)[0].2, "(delay 0)");
        assert!(lowered.diagnostics.is_empty());
    }

    #[test]
    fn delay_tuple_arity_outside_one_through_three_is_rejected() {
        for (delay, arity) in [("#()", 0), ("#(1, 2, 3, 4)", 4)] {
            let input = format!(
                "module sample(input logic a, output logic y); assign {delay} y = a; endmodule"
            );
            let error = lower_snippet(&input).unwrap_err();
            assert_eq!(
                error.message,
                format!("delay tuple must contain between one and three entries; got {arity}")
            );
        }
    }

    #[test]
    fn additional_specify_paths_are_one_strict_clean_ignore_at_the_second_path() {
        let lowered = lower_snippet(
            r#"module sample(input logic a, b, c, output logic y);
  assign y = a;
  assign y = b;
  specify
    (a *> y) = (T_first);
    (b *> y) = (T_second);
    (c *> y) = (T_third);
  endspecify
endmodule
"#,
        )
        .unwrap();
        assert_eq!(
            assignment_tuple_strings(&lowered),
            vec![
                ("y".into(), "a".into(), "(delay T_first)".into()),
                ("y".into(), "b".into(), "(delay T_first)".into()),
            ]
        );
        let [diagnostic] = lowered.diagnostics.as_slice() else {
            panic!("expected exactly one additional-path diagnostic")
        };
        assert_eq!(diagnostic.kind, DiagnosticKind::IntentionalIgnore);
        assert_eq!(diagnostic.span, Span::new("snippet.sv", 6, 5));
        assert_eq!(
            diagnostic.message,
            "additional control-dependent specify path for target `y` is intentionally ignored because delay-tuple lowering temporarily selects the first source-ordered path for the target"
        );
        assert!(!DiagnosticPolicy::new(false).is_failure(diagnostic));
        assert!(!DiagnosticPolicy::new(true).is_failure(diagnostic));
    }

    #[test]
    fn symbolic_precharge_and_high_z_tuples_preserve_every_component() {
        let lowered = lower_snippet(
            "module sample(input logic a, ena_n, output logic y0, y1);\n\
             bufif0 #(T_rise, T_Z, T_Z) (y0, a, ena_n);\n\
             assign #(T_Z, T_fall, T_off) y1 = a;\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_tuple_strings(&lowered),
            vec![
                (
                    "y0".into(),
                    "(bufif0 a ena_n)".into(),
                    "(delay T_rise T_Z T_Z)".into(),
                ),
                ("y1".into(), "a".into(), "(delay T_Z T_fall T_off)".into(),),
            ]
        );
        assert!(lowered.diagnostics.is_empty());
    }

    #[test]
    fn every_omitted_delay_tuple_component_is_an_error() {
        for (tuple, omitted_entry) in [("(, 2)", 1), ("(1, , 3)", 2), ("(1, 2, )", 3)] {
            let source = format!(
                "module sample(input logic a, output logic y); assign #{tuple} y = a; endmodule"
            );
            let error = lower_snippet(&source).unwrap_err();
            assert_eq!(error.span, Span::new("snippet.sv", 1, 55));
            assert_eq!(
                error.message,
                format!("explicitly omitted delay tuple entry {omitted_entry} is unsupported")
            );
        }
    }

    #[test]
    fn timing_aliases_resolve_forward_references_and_preserve_resistance_factors() {
        let lowered = lower_snippet(
            "module sample(input logic a, output logic y0, y1, y2, y3);\n\
             localparam realtime T_FORWARD = T_BASE + 1;\n\
             localparam realtime T_BASE = tpd_elmore(L_y, R_nmos_ohm(8*L_unit) * 2);\n\
             localparam realtime T_REAL = tpd_elmore(L_y, R_nmos_ohm(8*L_unit) * 1.5);\n\
             localparam realtime T_SUM = tpd_elmore(L_y, R_pmos_ohm(3*L_unit) + R_nmos_ohm(W_y*L_unit));\n\
             assign #(T_FORWARD) y0 = a;\n\
             assign #(T_REAL) y1 = a;\n\
             assign #(T_SUM) y2 = a;\n\
             assign #(tpd_z(, T_REAL, T_BASE)) y3 = a;\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "y0".into(),
                    "a".into(),
                    "(+ (elmore (wire L_y) (* (nmos 8) 2)) 1)".into(),
                ),
                (
                    "y1".into(),
                    "a".into(),
                    "(elmore (wire L_y) (* (nmos 8) 1.5))".into(),
                ),
                (
                    "y2".into(),
                    "a".into(),
                    "(elmore (wire L_y) (+ (pmos 3) (nmos W_y)))".into(),
                ),
                (
                    "y3".into(),
                    "a".into(),
                    "(elmore (wire L_y) (* (nmos 8) 1.5))".into(),
                ),
            ]
        );
        assert!(lowered.diagnostics.is_empty());
    }

    #[test]
    fn direct_real_resistance_factors_are_preserved() {
        let lowered = lower_snippet(
            "module sample(input logic a, output logic y0, y1);\n\
             assign #(tpd_elmore(L_y, R_pmos_ohm(13.5))) y0 = a;\n\
             assign #(tpd_elmore(L_y, R_nmos_ohm(10.8))) y1 = a;\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "y0".into(),
                    "a".into(),
                    "(elmore (wire L_y) (pmos 13.5))".into(),
                ),
                (
                    "y1".into(),
                    "a".into(),
                    "(elmore (wire L_y) (nmos 10.8))".into(),
                ),
            ]
        );
    }

    #[test]
    fn cyclic_timing_aliases_fail_deterministically() {
        let error = lower_snippet(
            "module sample(input logic a, output logic y);\n\
             localparam realtime T_B = T_A + 1;\n\
             localparam realtime T_A = T_B + 2;\n\
             assign #(T_A) y = a;\n\
             endmodule",
        )
        .unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 2, 27));
        assert_eq!(
            error.message,
            "cyclic timing alias dependency: T_A -> T_B -> T_A"
        );
    }

    #[test]
    fn uncontracted_value_operator_reports_its_source_span() {
        let error = lower_snippet(
            "module sample(input logic a, output logic y);\n  assign y = a + 1;\nendmodule",
        )
        .unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 2, 14));
        assert!(error.message.contains("not contracted value expressions"));
    }

    #[test]
    fn timing_clamp_uses_contracted_greater_and_mux_operators() {
        let lowered = lower_snippet(
            "module sample(input logic a, output logic y);\n  assign #((0.2 * T_fall_y1) > T_Z_min ? (0.2 * T_fall_y1) : T_Z_min) y = a;\nendmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered)[0].2,
            "(mux (gt (* 0.2 T_fall_y1) T_Z_min) (* 0.2 T_fall_y1) T_Z_min)"
        );
    }

    #[test]
    fn timing_less_than_reports_its_source_span() {
        let error = lower_snippet(
            "module sample(input logic a, output logic y);\n  assign #(a < 1) y = a;\nendmodule",
        )
        .unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 2, 12));
        assert!(error.message.contains("less-than"));
    }

    #[test]
    fn high_z_lowers_as_equality_or_a_root_continuous_driver_only() {
        let equality = lower_snippet(
            "module sample(input logic a, output logic y); assign y = a === 'z; endmodule",
        )
        .unwrap();
        assert_eq!(assignment_strings(&equality)[0].1, "(caseeq a z)");

        let direct =
            lower_snippet("module sample(input logic a, output logic y); assign y = 'z; endmodule")
                .unwrap_err();
        assert!(direct.message.contains("high-Z"));

        let tristate = lower_snippet(
            "module sample(input logic a, input logic s, output logic y); assign y = s ? a : 'z; endmodule",
        )
        .unwrap();
        assert_eq!(assignment_strings(&tristate)[0].1, "(bufif1 a s)");

        let nested = lower_snippet(
            "module sample(input logic a, input logic s, output logic y); assign y = !(s ? a : 'z); endmodule",
        )
        .unwrap_err();
        assert_eq!(nested.span, Span::new("snippet.sv", 1, 75));
        assert_eq!(
            nested.message,
            "high-Z ternary is legal only as the root value of a continuous driver"
        );
    }

    #[test]
    fn signal_valued_high_z_polarities_and_compound_operands_are_flat() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, c, d, ena, ena_n, input logic in, output tri logic y0, y1, y2);\n\
             assign y0 = ena ? in : 'z;\n\
             assign y1 = ena_n ? 'z : in;\n\
             assign (strong1, highz0) y2 = (a & b) ? (c | d) : 'z;\n\
             endmodule",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "y0".to_string(),
                    "(bufif1 in ena)".to_string(),
                    "0".to_string()
                ),
                (
                    "y1".to_string(),
                    "(bufif0 in ena_n)".to_string(),
                    "0".to_string()
                ),
                ("t0".to_string(), "(or c d)".to_string(), "0".to_string()),
                ("t1".to_string(), "(and a b)".to_string(), "0".to_string()),
                (
                    "y2".to_string(),
                    "(bufif1-strength t0 t1 strong1 highz0)".to_string(),
                    "0".to_string(),
                ),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn direct_bufif_accepts_literal_signal_and_compound_values() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, ena, output tri logic y0, y1, y2);\n\
             bufif0 (y0, '1, ena);\n\
             bufif1 (y1, a, ena);\n\
             bufif0 (pull1, highz0) (y2, a | b, ena & b);\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "y0".to_string(),
                    "(bufif0 1 ena)".to_string(),
                    "0".to_string()
                ),
                (
                    "y1".to_string(),
                    "(bufif1 a ena)".to_string(),
                    "0".to_string()
                ),
                ("t0".to_string(), "(or a b)".to_string(), "0".to_string()),
                ("t1".to_string(), "(and ena b)".to_string(), "0".to_string()),
                (
                    "y2".to_string(),
                    "(bufif0-strength t0 t1 pull1 highz0)".to_string(),
                    "0".to_string(),
                ),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn lowers_direct_transistor_kinds_without_normalizing_topology() {
        let lowered = lower_snippet(
            "module sample(input logic a, g, output logic yn, yp, yr);\n\
             nmos (yn, a, g);\n\
             pmos (yp, a, g);\n\
             rnmos (yr, a, g);\n\
             endmodule\n",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("yn".into(), "(nmos a g)".into(), "0".into()),
                ("yp".into(), "(pmos a g)".into(), "0".into()),
                ("yr".into(), "(rnmos a g)".into(), "0".into()),
            ]
        );
        assert!(lowered.diagnostics.is_empty());
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn transistor_compound_operands_flatten_source_then_gate_and_keep_drivers_ordered() {
        let lowered = lower_snippet(
            "module sample(input logic a, b, g, h, output logic y);\n\
             assign y = a;\n\
             nmos (y, a & b, g | h);\n\
             pmos (y, b, g);\n\
             endmodule\n",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("y".into(), "a".into(), "0".into()),
                ("t0".into(), "(and a b)".into(), "0".into()),
                ("t1".into(), "(or g h)".into(), "0".into()),
                ("y".into(), "(nmos t0 t1)".into(), "0".into()),
                ("y".into(), "(pmos b g)".into(), "0".into()),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn transistor_delays_preserve_explicit_tuple_or_complete_specify_fallback() {
        let lowered = lower_snippet(
            "module sample(input logic a, g, output logic explicit, fallback);\n\
             nmos #(D_first, D_later, D_off) (explicit, a, g);\n\
             pmos (fallback, a, g);\n\
             specify\n\
               (a *> explicit) = (S_explicit);\n\
               (a *> fallback) = (S_fallback);\n\
             endspecify\n\
             endmodule\n",
        )
        .unwrap();
        assert_eq!(
            assignment_tuple_strings(&lowered),
            vec![
                (
                    "explicit".into(),
                    "(nmos a g)".into(),
                    "(delay D_first D_later D_off)".into(),
                ),
                (
                    "fallback".into(),
                    "(pmos a g)".into(),
                    "(delay S_fallback)".into(),
                ),
            ]
        );
        assert!(lowered.diagnostics.is_empty());
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn transistor_shape_and_strength_diagnostics_are_precise() {
        let cases = [
            (
                "module sample(input logic a, g, output logic y);\n  nmos (y, a);\nendmodule\n",
                Span::new("snippet.sv", 2, 3),
                "expected nmos arity",
            ),
            (
                "module sample(input logic a, g, output logic y);\n  pmos (y, , g);\nendmodule\n",
                Span::new("snippet.sv", 2, 3),
                "expected pmos source argument",
            ),
            (
                "module sample(input logic a, g, output logic y);\n  nmos (y & a, a, g);\nendmodule\n",
                Span::new("snippet.sv", 2, 9),
                "expected nmos drain scalar symbol",
            ),
            (
                "module sample(input logic a, g, output logic y);\n  nmos (strong1, highz0) (y, a, g);\nendmodule\n",
                Span::new("snippet.sv", 2, 8),
                "strength-qualified nmos is unsupported because direct transistor value operators do not carry source strength",
            ),
        ];

        for (source, span, message) in cases {
            let diagnostic = lower_snippet(source).unwrap_err();
            assert_eq!(diagnostic.span, span, "{message}");
            assert_eq!(diagnostic.message, message);
        }
    }

    #[test]
    fn all_strength_pairs_and_driver_operators_preserve_source_atom_order() {
        let lowered = lower_snippet(
            "module sample(input logic a, ena, output tri logic y0, y1, y2, y3, y4, y5);\n\
             assign (strong1, highz0) y0 = a & ena;\n\
             assign (highz1, strong0) y1 = a;\n\
             assign (pull1, highz0) y2 = a;\n\
             assign (supply1, supply0) y3 = 1;\n\
             bufif0 (strong1, highz0) (y4, a, ena);\n\
             bufif1 (highz1, strong0) (y5, a, ena);\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("t0".to_string(), "(and a ena)".to_string(), "0".to_string()),
                (
                    "y0".to_string(),
                    "(drive-strength t0 strong1 highz0)".to_string(),
                    "0".to_string()
                ),
                (
                    "y1".to_string(),
                    "(drive-strength a highz1 strong0)".to_string(),
                    "0".to_string()
                ),
                (
                    "y2".to_string(),
                    "(drive-strength a pull1 highz0)".to_string(),
                    "0".to_string()
                ),
                (
                    "y3".to_string(),
                    "(drive-strength 1 supply1 supply0)".to_string(),
                    "0".to_string()
                ),
                (
                    "y4".to_string(),
                    "(bufif0-strength a ena strong1 highz0)".to_string(),
                    "0".to_string()
                ),
                (
                    "y5".to_string(),
                    "(bufif1-strength a ena highz1 strong0)".to_string(),
                    "0".to_string()
                ),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn invalid_strength_shapes_and_pairs_fail_at_the_strength_span() {
        for (values, expected) in [
            (
                "strong1",
                "drive strength must contain exactly two values; got 1: `(strong1)`",
            ),
            (
                "strong1, highz0, weak1",
                "drive strength must contain exactly two values; got 3: `(strong1, highz0, weak1)`",
            ),
            (
                "highz0, strong1",
                "unsupported drive strength pair `(highz0, strong1)`",
            ),
            (
                "weak1, highz0",
                "unsupported drive strength pair `(weak1, highz0)`",
            ),
        ] {
            let input = format!(
                "module sample(input logic a, output logic y);\n  assign ({values}) y = a;\nendmodule"
            );
            let error = lower_snippet(&input).unwrap_err();
            assert_eq!(error.span, Span::new("snippet.sv", 2, 10), "{values}");
            assert_eq!(error.message, expected, "{values}");
        }
    }

    #[test]
    fn repeated_precharge_and_open_drain_drivers_stay_separate_and_ordered() {
        let lowered = lower_snippet(
            "module sample(input logic pch_n, a, b, output tri logic y);\n\
             bufif0 (strong1, highz0) (y, '1, pch_n);\n\
             assign (highz1, strong0) y = a ? 0 : 'z;\n\
             assign (highz1, strong0) y = (a & b) ? 0 : 'z;\n\
             endmodule",
        )
        .unwrap();
        assert!(lowered.cell.registers.is_empty());
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                (
                    "y".to_string(),
                    "(bufif0-strength 1 pch_n strong1 highz0)".to_string(),
                    "0".to_string(),
                ),
                (
                    "y".to_string(),
                    "(bufif1-strength 0 a highz1 strong0)".to_string(),
                    "0".to_string(),
                ),
                ("t0".to_string(), "(and a b)".to_string(), "0".to_string()),
                (
                    "y".to_string(),
                    "(bufif1-strength 0 t0 highz1 strong0)".to_string(),
                    "0".to_string(),
                ),
            ]
        );
        lowered.cell.validate().unwrap();
    }

    #[test]
    fn bufif_shape_diagnostics_remain_precise() {
        let wrong_arity = lower_snippet(
            "module sample(input logic a, output tri logic y);\n  bufif0 (y, a);\nendmodule",
        )
        .unwrap_err();
        assert_eq!(wrong_arity.span, Span::new("snippet.sv", 2, 3));
        assert_eq!(wrong_arity.message, "expected bufif0 arity");

        let omitted = lower_snippet(
            "module sample(input logic a, output tri logic y);\n  bufif1 (y, , a);\nendmodule",
        )
        .unwrap_err();
        assert_eq!(omitted.span, Span::new("snippet.sv", 2, 3));
        assert_eq!(omitted.message, "expected bufif drive argument");

        let target = lower_snippet(
            "module sample(input logic a, b, output tri logic y);\n  bufif0 (y & b, a, b);\nendmodule",
        )
        .unwrap_err();
        assert_eq!(target.span, Span::new("snippet.sv", 2, 11));
        assert_eq!(target.message, "expected bufif target symbol");
    }

    #[test]
    fn timing_aware_lowering_records_actual_state_controls_and_preserves_compatibility() {
        let source = "module sample(input logic d, clk, reset_n, output logic q);\n  always_ff @(posedge clk, negedge reset_n) q <= d;\nendmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        let timing = lower_timing_snippet(source).unwrap();

        assert_eq!(timing.lowered(), &compatibility);
        assert!(compatibility.diagnostics.is_empty());
        assert_eq!(timing.assignment_provenance().len(), 1);
        let provenance = &timing.assignment_provenance()[0];
        assert_eq!(
            provenance.origin(),
            AssignmentOrigin::Source(SourceAssignmentOrigin::ProceduralStateful)
        );
        assert_eq!(
            provenance
                .state_controls()
                .iter()
                .map(|control| (control.signal(), control.transition(), control.span().line))
                .collect::<Vec<_>>(),
            vec![
                (Some("clk"), Some(Transition::Rise), 2),
                (Some("reset_n"), Some(Transition::Fall), 2),
            ]
        );

        let state_controls = timing
            .functional_graph()
            .dependencies()
            .iter()
            .filter(|dependency| {
                dependency.edge().kind() == crate::timing_graph::DependencyKind::StateControl
            })
            .collect::<Vec<_>>();
        assert_eq!(state_controls.len(), 2);
        assert_eq!(
            state_controls
                .iter()
                .map(|dependency| dependency.edge().event_transition())
                .collect::<Vec<_>>(),
            vec![Some(Transition::Rise), Some(Transition::Fall)]
        );
        assert_eq!(timing.cut_graph().excluded_state_boundaries().len(), 1);
        assert_eq!(
            timing
                .cut_graph()
                .dependencies()
                .iter()
                .filter(|dependency| dependency.edge().kind()
                    == crate::timing_graph::DependencyKind::StateControl)
                .count(),
            2
        );
    }

    #[test]
    fn timing_aware_path_rejects_non_scalar_state_events_without_changing_m14_lowering() {
        let source = "module sample(input logic d, clk, ena, output logic q);\n  always_ff @(posedge (clk & ena)) q <= d;\nendmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        assert!(compatibility.diagnostics.is_empty());

        let error = lower_timing_snippet(source).unwrap_err();
        assert_eq!(error.span.line, 2);
        assert_eq!(
            error.message,
            "stateful event control must be a scalar signal"
        );
    }

    #[test]
    fn a_clock_named_combinational_operand_is_not_a_state_control() {
        let source =
            "module sample(input logic clk, output logic q);\n  always @* q = clk;\nendmodule\n";
        let timing = lower_timing_snippet(source).unwrap();

        assert!(timing.lowered().cell.registers.is_empty());
        assert_eq!(
            timing.assignment_provenance()[0].origin(),
            AssignmentOrigin::Source(SourceAssignmentOrigin::ProceduralCombinational)
        );
        assert!(
            timing.assignment_provenance()[0]
                .state_controls()
                .is_empty()
        );
        assert!(
            timing
                .functional_graph()
                .dependencies()
                .iter()
                .all(|dependency| dependency.edge().kind()
                    != crate::timing_graph::DependencyKind::StateControl)
        );
    }

    #[test]
    fn generated_temporary_provenance_is_aligned_parented_and_graph_visible() {
        let source = "module sample(input logic a, b, c, d, output logic y);\n  assign y = (a & b) | (c & d);\nendmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        let timing = lower_timing_snippet(source).unwrap();
        assert_eq!(timing.lowered(), &compatibility);

        assert_eq!(timing.assignment_provenance().len(), 3);
        assert_eq!(
            timing
                .assignment_provenance()
                .iter()
                .map(AssignmentProvenance::assignment_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            timing
                .assignment_provenance()
                .iter()
                .map(AssignmentProvenance::source_assignment_order)
                .collect::<Vec<_>>(),
            vec![0, 0, 0]
        );
        assert!(timing.assignment_provenance()[0].origin().is_temporary());
        assert!(timing.assignment_provenance()[1].origin().is_temporary());
        assert_eq!(
            timing.assignment_provenance()[2].origin(),
            AssignmentOrigin::Source(SourceAssignmentOrigin::Continuous)
        );
        assert_eq!(timing.assignment_provenance()[0].span().line, 2);
        assert_eq!(timing.assignment_provenance()[1].span().line, 2);

        for temporary in ["t0", "t1"] {
            let id = timing.functional_graph().signal_id(temporary).unwrap();
            assert!(matches!(
                timing.functional_graph().node(id).unwrap().kind(),
                crate::timing_graph::TimingNodeKind::Signal(signal)
                    if signal.has_role(TimingSignalRole::Internal)
                        && signal.has_role(TimingSignalRole::Temporary)
            ));
        }
    }

    #[test]
    fn timing_signal_metadata_uses_only_typed_resolved_net_declarations() {
        let timing = lower_timing_snippet(
            "module sample(input logic il, input wire iw, output logic ol, output tri ot, inout logic io);\n\
  wire w;\n\
  tri t;\n\
  logic l;\n\
endmodule\n",
        )
        .unwrap();
        let roles = timing
            .functional_graph()
            .nodes()
            .filter_map(|node| match node.kind() {
                crate::timing_graph::TimingNodeKind::Signal(signal) => {
                    Some((signal.name(), signal.roles()))
                }
                crate::timing_graph::TimingNodeKind::Assignment(_) => None,
            })
            .collect::<BTreeMap<_, _>>();

        for resolved in ["iw", "ot", "io", "w", "t"] {
            assert!(
                roles[resolved].contains(&TimingSignalRole::ResolvedNet),
                "{resolved} lost its typed resolved-net classification"
            );
        }
        for variable in ["il", "ol", "l"] {
            assert!(
                !roles[variable].contains(&TimingSignalRole::ResolvedNet),
                "{variable} was inferred resolved without wire/tri/inout source semantics"
            );
        }
        assert!(roles["io"].contains(&TimingSignalRole::Inout));
    }

    #[test]
    fn timing_aware_lowering_captures_every_specify_path_tuple_and_exact_source_order() {
        let source = "module sample(input logic a, b, c, output logic y0, y1, y2, y3, y4, y5);\n\
  assign y0 = a; assign y1 = b; assign y2 = c;\n\
  assign y3 = a; assign y4 = b; assign y5 = c;\n\
  specify\n\
    (a *> y0) = (A0);\n\
  endspecify\n\
  specify\n\
    (b *> y1) = (B0, B1);\n\
    (c *> y2) = (C0, C1, C2);\n\
  endspecify\n\
  specify\n\
    (a *> y3) = (D0 + D1, D2);\n\
    (b *> y4) = ((E0 + E1) + E2, E3 + (E4 + E5), E6);\n\
    (c *> y5) = (F0);\n\
  endspecify\n\
endmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        let timing = lower_timing_snippet(source).unwrap();
        assert_eq!(timing.lowered(), &compatibility);

        let constraints = timing.functional_graph().constraints();
        assert_eq!(constraints.len(), 6);
        assert_eq!(
            constraints
                .iter()
                .map(|constraint| (
                    constraint.path_order(),
                    constraint.target(),
                    constraint.span().line,
                    constraint.target_span().line,
                    constraint
                        .controls()
                        .iter()
                        .map(|control| (control.source().signal(), control.source().span().line))
                        .collect::<Vec<_>>(),
                    render_delay_tuple(constraint.delay()),
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, "y0", 5, 5, vec![("a", 5)], "(delay A0)".to_string()),
                (1, "y1", 8, 8, vec![("b", 8)], "(delay B0 B1)".to_string()),
                (
                    2,
                    "y2",
                    9,
                    9,
                    vec![("c", 9)],
                    "(delay C0 C1 C2)".to_string()
                ),
                (
                    3,
                    "y3",
                    12,
                    12,
                    vec![("a", 12)],
                    "(delay (+ D0 D1) D2)".to_string()
                ),
                (
                    4,
                    "y4",
                    13,
                    13,
                    vec![("b", 13)],
                    "(delay (+ (+ E0 E1) E2) (+ E3 (+ E4 E5)) E6)".to_string()
                ),
                (5, "y5", 14, 14, vec![("c", 14)], "(delay F0)".to_string()),
            ]
        );
        for constraint in constraints {
            assert_eq!(
                constraint.additive_delay().to_delay_tuple().unwrap(),
                *constraint.delay()
            );
        }
        assert_eq!(
            constraints[4]
                .additive_delay()
                .components()
                .map(|component| {
                    component
                        .terms()
                        .iter()
                        .map(|term| render_timing_expr(term.as_timing_expr()))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            vec![
                vec!["E0".to_string(), "E1".to_string(), "E2".to_string()],
                vec!["E3".to_string(), "E4".to_string(), "E5".to_string()],
                vec!["E6".to_string()],
            ]
        );
    }

    #[test]
    fn timing_aware_unreachable_control_is_exact_without_changing_ordinary_lowering() {
        let source = "module sample(input logic a, unused, output logic y);\n\
  assign y = a;\n\
  specify\n\
    (unused *> y) = (T);\n\
  endspecify\n\
endmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        assert!(compatibility.diagnostics.is_empty());

        let error = lower_timing_snippet(source).unwrap_err();
        assert_eq!(error.span, Span::new("snippet.sv", 4, 2));
        assert_eq!(
            error.message,
            "timing constraint p0 control c0 `unused` cannot reach target `y` in the full functional graph"
        );
    }

    #[test]
    fn full_graph_keeps_state_boundary_reachability_and_reports_event_transition() {
        let source = "module sample(input logic d, clk, output logic q);\n\
  always_ff @(posedge clk) q <= d;\n\
  specify\n\
    (clk *> q) = (T_rise, T_fall);\n\
  endspecify\n\
endmodule\n";
        let timing = lower_timing_snippet(source).unwrap();
        assert_eq!(timing.cut_graph().excluded_state_boundaries().len(), 1);
        let report = &timing.timing_analysis().target_groups()[0].control_reports()[0];
        assert_eq!(
            report.path_senses(),
            &[crate::timing_graph::TimingPathSense::StateControl {
                event_transition: Some(Transition::Rise),
                target_effect: Some(crate::timing_graph::TransitionEffect::Exact(
                    Transition::Rise
                )),
            }]
        );
        assert!(report.reachable_nodes().contains(&report.target_node()));
    }

    #[test]
    fn timing_analysis_reports_all_functional_sense_classes_and_public_splits() {
        let source = "module sample(input logic a, b, ena, clk, output logic yp, yn, yx, yc, q, derived);\n\
  logic internal;\n\
  assign yp = a;\n\
  assign yn = ~a;\n\
  assign yx = a ^ b;\n\
  assign yc = ena ? a : 'z;\n\
  always_ff @(posedge clk) q <= a;\n\
  assign derived = ~yp;\n\
  assign internal = a;\n\
  specify\n\
    (a *> yp) = (TP);\n\
    (a *> yn) = (TN);\n\
    (a *> yx) = (TX);\n\
    (ena *> yc) = (TC);\n\
    (clk *> q) = (TQ);\n\
    (a *> internal) = (TI);\n\
  endspecify\n\
endmodule\n";
        let timing = lower_timing_snippet(source).unwrap();
        let groups = timing
            .timing_analysis()
            .target_groups()
            .iter()
            .map(|report| (report.group().target(), report))
            .collect::<BTreeMap<_, _>>();
        let senses = |target: &str| groups[target].control_reports()[0].path_senses().to_vec();
        assert_eq!(
            senses("yp"),
            vec![crate::timing_graph::TimingPathSense::PositiveUnate]
        );
        assert_eq!(
            senses("yn"),
            vec![crate::timing_graph::TimingPathSense::NegativeUnate]
        );
        assert_eq!(
            senses("yx"),
            vec![crate::timing_graph::TimingPathSense::NonUnate]
        );
        assert_eq!(
            senses("yc"),
            vec![crate::timing_graph::TimingPathSense::Conditional]
        );
        assert!(matches!(
            senses("q").as_slice(),
            [crate::timing_graph::TimingPathSense::StateControl {
                event_transition: Some(Transition::Rise),
                ..
            }]
        ));
        assert_eq!(
            groups["yp"].public_output_split(),
            crate::timing_graph::PublicOutputSplit::Candidate
        );
        assert_eq!(
            groups["yn"].public_output_split(),
            crate::timing_graph::PublicOutputSplit::NotRequired
        );
        assert_eq!(
            groups["internal"].public_output_split(),
            crate::timing_graph::PublicOutputSplit::NotPublic
        );
    }

    #[test]
    fn decomposed_timing_is_opt_in_exact_and_erasable() {
        let source = "module sample(input logic a, output logic y, z);\n\
  assign y = a;\n\
  assign z = ~y;\n\
  specify\n\
    (a *> y) = (T_prefix + T_y);\n\
    (a *> z) = (T_prefix + T_y + T_z);\n\
  endspecify\n\
endmodule\n";
        let compatibility = lower_snippet(source).unwrap();
        let decomposed = lower_decomposed_timing_snippet(source).unwrap();
        assert_eq!(compatibility.diagnostics.len(), 0);
        assert_eq!(
            decomposed
                .strategy()
                .exact_cover()
                .unwrap()
                .2
                .checked_placements()
                .len(),
            decomposed
                .strategy()
                .exact_cover()
                .unwrap()
                .0
                .placements()
                .len()
        );
        assert!(
            decomposed
                .assignment_provenance()
                .iter()
                .any(|provenance| provenance.delay_origin()
                    == AssignmentDelayOrigin::DecompositionPlacement)
        );
        let erased = decomposed
            .erasure()
            .erase(decomposed.lowered(), decomposed.assignment_provenance())
            .unwrap();
        let baseline_design = crate::parser::parse_file(Path::new("snippet.sv"), source).unwrap();
        let elaborated = elaborate_design(&baseline_design, GenerateMode::default()).unwrap();
        let analysis = analyze_design_structural(&elaborated);
        let baseline = lower_elaborated_design_artifacts_with_policy(
            &elaborated,
            &analysis,
            TimingLoweringPolicy::DecompositionBaseline,
        )
        .unwrap();
        assert_eq!(erased.lowered(), &baseline.lowered);
        assert_eq!(
            erased.assignment_provenance(),
            baseline.assignment_provenance
        );
    }

    #[test]
    fn decomposed_timing_smoke_covers_ao21() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sv-cells/dmg_cpu_b/cells/ao21.sv");
        let input = fs::read_to_string(&path).unwrap();
        let design = crate::parser::parse_file(&path, &input).unwrap();
        let first =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap();
        let second =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap();
        assert!(first.decomposition().is_some());
        assert!(!first.is_physical_topology());
        assert!(first.strategy().physical_topology().is_none());
        assert_eq!(first.strategy(), second.strategy());
        assert_eq!(first.lowered(), second.lowered());
        assert_eq!(
            first.assignment_provenance(),
            second.assignment_provenance()
        );
        assert_eq!(first.signal_metadata(), second.signal_metadata());
        assert_eq!(
            crate::serialize::render_cell(&first.lowered().cell),
            crate::serialize::render_cell(&second.lowered().cell)
        );
        assert_eq!(
            first
                .strategy()
                .exact_cover()
                .unwrap()
                .2
                .checked_placements()
                .len(),
            first.strategy().exact_cover().unwrap().0.placements().len()
        );
        assert!(first.lowered().diagnostics.is_empty());
        let erased = first
            .erasure()
            .erase(first.lowered(), first.assignment_provenance())
            .unwrap();
        let elaborated = elaborate_design(&design, GenerateMode::Delayful).unwrap();
        let analysis = analyze_design_structural(&elaborated);
        let baseline = lower_elaborated_design_artifacts_with_policy(
            &elaborated,
            &analysis,
            TimingLoweringPolicy::DecompositionBaseline,
        )
        .unwrap();
        assert_eq!(erased.lowered(), &baseline.lowered);
        assert_eq!(
            erased.assignment_provenance(),
            baseline.assignment_provenance
        );
        assert_eq!(erased.signal_metadata(), baseline.signal_metadata);
    }

    #[test]
    fn dmg_dffsr_builtin_topology_resolves_materializes_rebuilds_and_erases() {
        use crate::topology_apply::materialize_topology;
        use crate::topology_hint::{
            DMG_DFFSR_HINT_PATH, TopologyHintCatalog, TopologyHintContext,
            builtin_dmg_dffsr_hint_source, builtin_topology_hint_catalog,
        };
        use crate::topology_verify::verify_materialized_topology;

        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sv-cells/dmg_cpu_b/cells/dffsr.sv");
        let input = fs::read_to_string(&path).unwrap();
        let design = crate::parser::parse_file(&path, &input).unwrap();
        let elaborated = elaborate_design(&design, GenerateMode::Delayful).unwrap();
        let analysis = analyze_design_structural(&elaborated);
        let baseline = lower_elaborated_design_artifacts_with_policy(
            &elaborated,
            &analysis,
            TimingLoweringPolicy::DecompositionBaseline,
        )
        .unwrap();
        let graph = build_timing_graph(
            &baseline.lowered.cell,
            &baseline.signal_metadata,
            &baseline.assignment_provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        assert_eq!(graph.constraints().len(), 6);
        assert!(
            graph
                .constraints()
                .iter()
                .all(|constraint| constraint.controls().len() == 1)
        );

        let hint_source = builtin_dmg_dffsr_hint_source();
        let line_of = |text: &str, marker: &str| {
            text[..text.find(marker).unwrap()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1
        };
        let corruptions = [
            (
                "anchor",
                hint_source.replacen(
                    "target = \"q\", expression = { operator = \"mux\", operands = [\"t9\", \"t12\", \"q\"] }",
                    "target = \"q\", expression = { atom = \"stale_q\" }",
                    1,
                ),
                "stale_q",
                "baseline state role",
            ),
            (
                "alias",
                hint_source.replacen("T_rise_not2", "MISSING_ALIAS", 1),
                "MISSING_ALIAS",
                "not a resolved alias or specparam",
            ),
            (
                "step",
                hint_source.replacen(
                    "assignment = { generated = \"fast_mux\" }, operand_index = 0, transition = \"rise\"",
                    "assignment = { generated = \"fast_mux\" }, operand_index = 1, transition = \"rise\"",
                    1,
                ),
                "generated = \"fast_mux\" }, operand_index = 1",
                "discontinuous dependency walk",
            ),
            (
                "q_n reset delay",
                hint_source.replacen(
                    "rise = [\"T_fall_not1\"], fall = [\"T_rise_not1\"]",
                    "rise = [\"MISSING_Q_N_RESET\"], fall = [\"T_rise_not1\"]",
                    1,
                ),
                "MISSING_Q_N_RESET",
                "not a resolved alias or specparam",
            ),
            (
                "q_n route through q",
                hint_source.replacen(
                    "assignment = { generated = \"physical_q_n\" }, operand_index = 0, transition = \"rise\"",
                    "assignment = { generated = \"physical_q\" }, operand_index = 0, transition = \"rise\"",
                    1,
                ),
                "id = \"p3_clk_q_n_rise\"",
                "discontinuous dependency walk",
            ),
        ];
        for (name, text, marker, message) in corruptions {
            let error = TopologyHintCatalog::parse(DMG_DFFSR_HINT_PATH, &text)
                .unwrap()
                .resolve(&TopologyHintContext::new(
                    "dmg_dffsr",
                    GenerateMode::Delayful,
                    &baseline.lowered,
                    &graph,
                ))
                .unwrap_err();
            assert_eq!(error.span().path, Path::new(DMG_DFFSR_HINT_PATH), "{name}");
            assert_eq!(error.span().line, line_of(&text, marker), "{name}");
            assert!(
                error.message().contains(message),
                "{name}: {}",
                error.message()
            );
        }

        let catalog = builtin_topology_hint_catalog().unwrap();
        let resolved = catalog
            .resolve(&TopologyHintContext::new(
                "dmg_dffsr",
                GenerateMode::Delayful,
                &baseline.lowered,
                &graph,
            ))
            .unwrap();
        let hint = &resolved.hints()[0];
        assert_eq!(hint.constraint_paths().len(), 6);
        assert_eq!(hint.alias_terms().len(), 16);

        let recipe_steps = |recipe: &crate::topology_hint::ResolvedPathRecipe| {
            recipe
                .steps
                .iter()
                .filter_map(|step| match &step.kind {
                    crate::topology_hint::ResolvedPathStepKind::Baseline(_) => None,
                    crate::topology_hint::ResolvedPathStepKind::Generated(id) => Some((
                        format!("generated:{}", id.as_str()),
                        step.operand_index,
                        step.transition,
                    )),
                    crate::topology_hint::ResolvedPathStepKind::Rewrite(id) => Some((
                        format!("rewrite:{}", id.as_str()),
                        step.operand_index,
                        step.transition,
                    )),
                })
                .collect::<Vec<_>>()
        };
        let expected_recipes = [
            (
                "p0_clk_q_rise",
                0,
                "q",
                Transition::Rise,
                vec![
                    ("generated:clock_inv", 0, Transition::Fall),
                    ("generated:fast_mux", 0, Transition::Rise),
                    ("generated:selector_mux", 1, Transition::Rise),
                    ("generated:not4", 0, Transition::Fall),
                    ("generated:physical_q", 0, Transition::Rise),
                    ("generated:q_replacement", 1, Transition::Rise),
                    ("rewrite:q_state", 0, Transition::Rise),
                ],
                vec!["T_fall_not2", "T_rise_mux", "T_fall_not4", "T_rise_q"],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p0_clk_q_fall",
                0,
                "q",
                Transition::Fall,
                vec![
                    ("generated:clock_inv", 0, Transition::Fall),
                    ("generated:restored_clock", 0, Transition::Rise),
                    ("generated:slow_mux", 0, Transition::Fall),
                    ("generated:selector_mux", 2, Transition::Fall),
                    ("generated:not4", 0, Transition::Rise),
                    ("generated:physical_q", 0, Transition::Fall),
                    ("generated:q_replacement", 1, Transition::Fall),
                    ("rewrite:q_state", 0, Transition::Fall),
                ],
                vec![
                    "T_fall_not2",
                    "T_rise_not3",
                    "T_fall_mux",
                    "T_rise_not4",
                    "T_fall_q",
                ],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p1_s_q_rise",
                1,
                "q",
                Transition::Rise,
                vec![
                    ("generated:master_gate", 1, Transition::Fall),
                    ("generated:master_aoi", 1, Transition::Rise),
                    ("generated:fast_mux", 2, Transition::Rise),
                    ("generated:selector_mux", 1, Transition::Rise),
                    ("generated:not4", 0, Transition::Fall),
                    ("generated:physical_q", 0, Transition::Rise),
                    ("generated:q_replacement", 1, Transition::Rise),
                    ("rewrite:q_state", 0, Transition::Rise),
                ],
                vec!["T_rise_aoi", "T_rise_mux", "T_fall_not4", "T_rise_q"],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p1_s_q_fall",
                1,
                "q",
                Transition::Fall,
                vec![
                    ("generated:master_gate", 1, Transition::Rise),
                    ("generated:master_aoi", 1, Transition::Fall),
                    ("generated:slow_mux", 1, Transition::Fall),
                    ("generated:selector_mux", 2, Transition::Fall),
                    ("generated:not4", 0, Transition::Rise),
                    ("generated:physical_q", 0, Transition::Fall),
                    ("generated:q_replacement", 1, Transition::Fall),
                    ("rewrite:q_state", 0, Transition::Fall),
                ],
                vec!["T_fall_aoi", "T_fall_mux", "T_rise_not4", "T_fall_q"],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p2_r_q_rise",
                2,
                "q",
                Transition::Rise,
                vec![
                    ("generated:q_reset_inv", 0, Transition::Fall),
                    ("generated:master_aoi", 0, Transition::Rise),
                    ("generated:fast_mux", 2, Transition::Rise),
                    ("generated:selector_mux", 1, Transition::Rise),
                    ("generated:not4", 0, Transition::Fall),
                    ("generated:physical_q", 0, Transition::Rise),
                    ("generated:q_replacement", 1, Transition::Rise),
                    ("rewrite:q_state", 0, Transition::Rise),
                ],
                vec![
                    "T_fall_not1",
                    "T_rise_aoi",
                    "T_rise_mux",
                    "T_fall_not4",
                    "T_rise_q",
                ],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p2_r_q_fall",
                2,
                "q",
                Transition::Fall,
                vec![
                    ("generated:q_reset_inv", 0, Transition::Rise),
                    ("generated:master_aoi", 0, Transition::Fall),
                    ("generated:slow_mux", 1, Transition::Fall),
                    ("generated:selector_mux", 2, Transition::Fall),
                    ("generated:not4", 0, Transition::Rise),
                    ("generated:physical_q", 0, Transition::Fall),
                    ("generated:q_replacement", 1, Transition::Fall),
                    ("rewrite:q_state", 0, Transition::Fall),
                ],
                vec![
                    "T_rise_not1",
                    "T_fall_aoi",
                    "T_fall_mux",
                    "T_rise_not4",
                    "T_fall_q",
                ],
                vec!["q_known_guard", "q_fallback_guard"],
            ),
            (
                "p3_clk_q_n_rise",
                3,
                "q_n",
                Transition::Rise,
                vec![
                    ("generated:clock_inv", 0, Transition::Fall),
                    ("generated:restored_clock", 0, Transition::Rise),
                    ("generated:slow_mux", 0, Transition::Fall),
                    ("generated:selector_mux", 2, Transition::Fall),
                    ("generated:not4", 0, Transition::Rise),
                    ("generated:poststate_gate", 0, Transition::Rise),
                    ("generated:poststate_aoi", 1, Transition::Fall),
                    ("generated:physical_q_n", 0, Transition::Rise),
                    ("generated:q_n_replacement", 1, Transition::Rise),
                    ("rewrite:q_n_output", 0, Transition::Rise),
                ],
                vec![
                    "T_fall_not2",
                    "T_rise_not3",
                    "T_fall_mux",
                    "T_rise_not4",
                    "T_fall_aoi",
                    "T_rise_q_n",
                ],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
            (
                "p3_clk_q_n_fall",
                3,
                "q_n",
                Transition::Fall,
                vec![
                    ("generated:clock_inv", 0, Transition::Fall),
                    ("generated:fast_mux", 0, Transition::Rise),
                    ("generated:selector_mux", 1, Transition::Rise),
                    ("generated:not4", 0, Transition::Fall),
                    ("generated:poststate_gate", 0, Transition::Fall),
                    ("generated:poststate_aoi", 1, Transition::Rise),
                    ("generated:physical_q_n", 0, Transition::Fall),
                    ("generated:q_n_replacement", 1, Transition::Fall),
                    ("rewrite:q_n_output", 0, Transition::Fall),
                ],
                vec![
                    "T_fall_not2",
                    "T_rise_mux",
                    "T_fall_not4",
                    "T_rise_aoi",
                    "T_fall_q_n",
                ],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
            (
                "p4_s_q_n_rise",
                4,
                "q_n",
                Transition::Rise,
                vec![
                    ("generated:poststate_gate", 1, Transition::Rise),
                    ("generated:poststate_aoi", 1, Transition::Fall),
                    ("generated:physical_q_n", 0, Transition::Rise),
                    ("generated:q_n_replacement", 1, Transition::Rise),
                    ("rewrite:q_n_output", 0, Transition::Rise),
                ],
                vec!["T_fall_aoi", "T_rise_q_n"],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
            (
                "p4_s_q_n_fall",
                4,
                "q_n",
                Transition::Fall,
                vec![
                    ("generated:poststate_gate", 1, Transition::Fall),
                    ("generated:poststate_aoi", 1, Transition::Rise),
                    ("generated:physical_q_n", 0, Transition::Fall),
                    ("generated:q_n_replacement", 1, Transition::Fall),
                    ("rewrite:q_n_output", 0, Transition::Fall),
                ],
                vec!["T_rise_aoi", "T_fall_q_n"],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
            (
                "p5_r_q_n_rise",
                5,
                "q_n",
                Transition::Rise,
                vec![
                    ("generated:q_n_reset_view", 0, Transition::Rise),
                    ("generated:poststate_aoi", 0, Transition::Fall),
                    ("generated:physical_q_n", 0, Transition::Rise),
                    ("generated:q_n_replacement", 1, Transition::Rise),
                    ("rewrite:q_n_output", 0, Transition::Rise),
                ],
                vec!["T_fall_not1", "T_fall_aoi", "T_rise_q_n"],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
            (
                "p5_r_q_n_fall",
                5,
                "q_n",
                Transition::Fall,
                vec![
                    ("generated:q_n_reset_view", 0, Transition::Fall),
                    ("generated:poststate_aoi", 0, Transition::Rise),
                    ("generated:physical_q_n", 0, Transition::Fall),
                    ("generated:q_n_replacement", 1, Transition::Fall),
                    ("rewrite:q_n_output", 0, Transition::Fall),
                ],
                vec!["T_rise_not1", "T_rise_aoi", "T_fall_q_n"],
                vec!["q_n_known_guard", "q_n_fallback_guard"],
            ),
        ];
        assert_eq!(hint.recipes().len(), expected_recipes.len());
        for (recipe, (id, path_order, target, transition, steps, terms, guards)) in
            hint.recipes().iter().zip(expected_recipes)
        {
            assert_eq!(recipe.id.as_str(), id);
            assert_eq!(
                (
                    recipe.path_order,
                    recipe.control_order,
                    recipe.target.as_str(),
                    recipe.transition
                ),
                (path_order, 0, target, transition)
            );
            assert_eq!(
                recipe_steps(recipe),
                steps
                    .into_iter()
                    .map(|(id, operand, transition)| (id.into(), operand, transition))
                    .collect::<Vec<_>>()
            );
            assert_eq!(recipe.expected_terms.terms(), terms);
            assert_eq!(
                recipe
                    .omitted_guards
                    .iter()
                    .map(|guard| guard.as_str())
                    .collect::<Vec<_>>(),
                guards
            );
        }

        for recipe in hint
            .recipes()
            .iter()
            .filter(|recipe| recipe.target == "q_n")
        {
            let steps = recipe_steps(recipe);
            assert!(
                !steps
                    .iter()
                    .any(|(id, _, _)| id == "generated:physical_q" || id == "rewrite:q_state")
            );
            assert!(
                steps
                    .iter()
                    .any(|(id, _, _)| id == "generated:physical_q_n")
            );
            assert!(steps.iter().any(|(id, _, _)| id == "rewrite:q_n_output"));
        }

        let timing = |name| baseline.lowered.timing_aliases[name].clone();
        let zero = DelayTuple::One(TimingExpr::atom("0").unwrap());
        let expected_delays = [
            (
                "q_reset_inv",
                DelayTuple::Two {
                    rise: timing("T_rise_not1"),
                    fall: timing("T_fall_not1"),
                },
            ),
            (
                "q_n_reset_view",
                DelayTuple::Two {
                    rise: timing("T_fall_not1"),
                    fall: timing("T_rise_not1"),
                },
            ),
            (
                "clock_inv",
                DelayTuple::Two {
                    rise: timing("T_rise_not2"),
                    fall: timing("T_fall_not2"),
                },
            ),
            (
                "restored_clock",
                DelayTuple::Two {
                    rise: timing("T_rise_not3"),
                    fall: timing("T_fall_not3"),
                },
            ),
            (
                "fast_mux",
                DelayTuple::Two {
                    rise: timing("T_rise_mux"),
                    fall: TimingExpr::atom("0").unwrap(),
                },
            ),
            (
                "slow_mux",
                DelayTuple::Two {
                    rise: TimingExpr::atom("0").unwrap(),
                    fall: timing("T_fall_mux"),
                },
            ),
            ("selector_mux", zero.clone()),
            (
                "not4",
                DelayTuple::Two {
                    rise: timing("T_rise_not4"),
                    fall: timing("T_fall_not4"),
                },
            ),
            (
                "physical_q",
                DelayTuple::Two {
                    rise: timing("T_rise_q"),
                    fall: timing("T_fall_q"),
                },
            ),
            (
                "poststate_aoi",
                DelayTuple::Two {
                    rise: timing("T_rise_aoi"),
                    fall: timing("T_fall_aoi"),
                },
            ),
            (
                "physical_q_n",
                DelayTuple::Two {
                    rise: timing("T_rise_q_n"),
                    fall: timing("T_fall_q_n"),
                },
            ),
        ];
        for (id, delay) in expected_delays {
            assert_eq!(
                hint.assignments()
                    .iter()
                    .find(|assignment| assignment.id().as_str() == id)
                    .unwrap()
                    .delay(),
                &delay,
                "{id}"
            );
        }

        let value =
            |operator, operands: &[&str]| crate::topology_hint::TopologyValueExpr::Operation {
                operator,
                operands: operands.iter().map(|operand| (*operand).into()).collect(),
            };
        let expected_known_cone = [
            (
                "known_clk_zero",
                value(ValueOperator::CaseEq, &["clk_buf", "0"]),
            ),
            (
                "known_clk_one",
                value(ValueOperator::CaseEq, &["clk_buf", "1"]),
            ),
            (
                "known_s_zero",
                value(ValueOperator::CaseEq, &["s_n_buf", "0"]),
            ),
            (
                "known_s_one",
                value(ValueOperator::CaseEq, &["s_n_buf", "1"]),
            ),
            (
                "known_r_zero",
                value(ValueOperator::CaseEq, &["r_n_buf", "0"]),
            ),
            (
                "known_r_one",
                value(ValueOperator::CaseEq, &["r_n_buf", "1"]),
            ),
            ("known_ff_zero", value(ValueOperator::CaseEq, &["ff", "0"])),
            ("known_ff_one", value(ValueOperator::CaseEq, &["ff", "1"])),
            ("known_q_zero", value(ValueOperator::CaseEq, &["q", "0"])),
            ("known_q_one", value(ValueOperator::CaseEq, &["q", "1"])),
            (
                "known_clk",
                value(
                    ValueOperator::Or,
                    &["top_known_clk_zero", "top_known_clk_one"],
                ),
            ),
            (
                "known_s",
                value(ValueOperator::Or, &["top_known_s_zero", "top_known_s_one"]),
            ),
            (
                "known_r",
                value(ValueOperator::Or, &["top_known_r_zero", "top_known_r_one"]),
            ),
            (
                "known_ff",
                value(
                    ValueOperator::Or,
                    &["top_known_ff_zero", "top_known_ff_one"],
                ),
            ),
            (
                "known_q",
                value(ValueOperator::Or, &["top_known_q_zero", "top_known_q_one"]),
            ),
            (
                "all_known",
                value(
                    ValueOperator::And,
                    &[
                        "top_known_clk",
                        "top_known_s",
                        "top_known_r",
                        "top_known_ff",
                        "top_known_q",
                    ],
                ),
            ),
        ];
        for (id, expression) in expected_known_cone {
            assert_eq!(
                hint.assignments()
                    .iter()
                    .find(|assignment| assignment.id().as_str() == id)
                    .unwrap()
                    .expression(),
                &expression,
                "{id}"
            );
        }
        for (id, expression) in [
            ("q_fallback", value(ValueOperator::Mux, &["t9", "t12", "q"])),
            (
                "q_replacement",
                value(
                    ValueOperator::Mux,
                    &["top_all_known", "top_physical_q", "top_q_fallback"],
                ),
            ),
            ("q_n_fallback", value(ValueOperator::Not, &["q"])),
            (
                "q_n_replacement",
                value(
                    ValueOperator::Mux,
                    &["top_all_known", "top_physical_q_n", "top_q_n_fallback"],
                ),
            ),
        ] {
            assert_eq!(
                hint.assignments()
                    .iter()
                    .find(|assignment| assignment.id().as_str() == id)
                    .unwrap()
                    .expression(),
                &expression,
                "{id}"
            );
        }
        assert_eq!(
            hint.guards()
                .iter()
                .map(|guard| {
                    (
                        guard.id.as_str(),
                        guard.assignment.as_str(),
                        guard.operand_index,
                        guard.reason,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "q_known_guard",
                    "q_replacement",
                    0,
                    crate::topology_hint::RoutingGuardReason::Knownness,
                ),
                (
                    "q_fallback_guard",
                    "q_replacement",
                    2,
                    crate::topology_hint::RoutingGuardReason::ExactFallback,
                ),
                (
                    "q_n_known_guard",
                    "q_n_replacement",
                    0,
                    crate::topology_hint::RoutingGuardReason::Knownness,
                ),
                (
                    "q_n_fallback_guard",
                    "q_n_replacement",
                    2,
                    crate::topology_hint::RoutingGuardReason::ExactFallback,
                ),
            ]
        );

        let transformed = materialize_topology(
            hint.require_materialization(),
            &baseline.lowered,
            &baseline.signal_metadata,
            &baseline.assignment_provenance,
        )
        .unwrap();
        let mut applied = transformed.facts.assignments.values().collect::<Vec<_>>();
        applied.sort_by_key(|assignment| assignment.item_order);
        assert_eq!(
            applied
                .iter()
                .map(|assignment| assignment.id.as_str())
                .collect::<Vec<_>>(),
            hint.assignments()
                .iter()
                .map(|assignment| assignment.id().as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            applied
                .iter()
                .map(|assignment| assignment.assignment.target.as_str())
                .collect::<Vec<_>>(),
            hint.signals()
                .iter()
                .map(|signal| signal.name())
                .collect::<Vec<_>>()
        );
        for assignment in &applied {
            assert_eq!(
                transformed
                    .provenance
                    .get(assignment.assignment_order)
                    .unwrap()
                    .origin(),
                AssignmentOrigin::GeneratedTopology {
                    parent: transformed
                        .provenance
                        .get(assignment.assignment_order)
                        .unwrap()
                        .origin()
                        .source(),
                }
            );
            assert_eq!(
                transformed
                    .provenance
                    .get(assignment.assignment_order)
                    .unwrap()
                    .delay_origin(),
                AssignmentDelayOrigin::TopologyPlacement
            );
            assert_eq!(
                transformed
                    .lowered
                    .cell
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        CellItem::Assignment(candidate)
                            if candidate.target == assignment.assignment.target =>
                        {
                            Some(candidate)
                        }
                        _ => None,
                    })
                    .count(),
                1,
                "{} has one generated driver",
                assignment.id
            );
        }
        for id in [
            "q_reset_inv",
            "q_n_reset_view",
            "clock_inv",
            "restored_clock",
            "fast_mux",
            "slow_mux",
            "not4",
            "physical_q",
            "poststate_aoi",
            "physical_q_n",
        ] {
            let fact = applied
                .iter()
                .find(|assignment| assignment.id.as_str() == id)
                .unwrap();
            assert_ne!(fact.assignment.delay, zero, "{id} retains physical delay");
        }
        assert_eq!(
            transformed
                .facts
                .rewrites
                .values()
                .map(|rewrite| {
                    (
                        rewrite.baseline.as_str(),
                        rewrite.before.target.as_str(),
                        &rewrite.before.expr,
                        rewrite.after.target.as_str(),
                        &rewrite.after.expr,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "q_n_output",
                    "q_n",
                    &Expr::value(ValueOperator::Not, vec![Expr::atom("q")]),
                    "q_n",
                    &Expr::atom("top_q_n_replacement"),
                ),
                (
                    "q_state",
                    "q",
                    &Expr::value(
                        ValueOperator::Mux,
                        vec![Expr::atom("t9"), Expr::atom("t12"), Expr::atom("q")],
                    ),
                    "q",
                    &Expr::atom("top_q_replacement"),
                ),
            ]
        );
        assert_eq!(
            transformed
                .lowered
                .cell
                .registers
                .iter()
                .map(|register| (register.name.as_str(), register.initial))
                .collect::<Vec<_>>(),
            vec![("ff", LogicValue::Zero), ("q", LogicValue::Zero)]
        );
        let rebuilt = build_timing_graph(
            &transformed.lowered.cell,
            &transformed.metadata,
            &transformed.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let verification = verify_materialized_topology(hint, &transformed, &rebuilt).unwrap();
        assert_eq!(verification.paths().len(), 12);
        assert_eq!(
            verification
                .paths()
                .iter()
                .map(|path| path.recipe())
                .collect::<Vec<_>>(),
            hint.recipes()
                .iter()
                .map(|recipe| recipe.id.as_str())
                .collect::<Vec<_>>()
        );
        for path in verification
            .paths()
            .iter()
            .filter(|path| path.recipe().contains("q_n"))
        {
            assert!(!path.steps().iter().any(|step| matches!(
                step.kind(),
                crate::topology_hint::ResolvedPathStepKind::Generated(id)
                    if id.as_str() == "physical_q"
            )));
            assert!(!path.steps().iter().any(|step| matches!(
                step.kind(),
                crate::topology_hint::ResolvedPathStepKind::Rewrite(id)
                    if id.as_str() == "q_state"
            )));
        }
        let source_components = verification
            .paths()
            .iter()
            .map(|path| {
                (
                    path.constraint().ordinal(),
                    path.control().ordinal(),
                    path.target_transition(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            source_components,
            (0..6)
                .flat_map(|path| {
                    [Transition::Rise, Transition::Fall]
                        .into_iter()
                        .map(move |transition| (path, path, transition))
                })
                .collect()
        );
        for path in verification.paths() {
            let constraint = &rebuilt.constraints()[path.constraint().ordinal() as usize];
            let component = match path.target_transition() {
                Transition::Rise => constraint.additive_delay().component(0).unwrap(),
                Transition::Fall => constraint.additive_delay().component(1).unwrap(),
                Transition::TurnOff => unreachable!(),
            };
            assert_eq!(
                path.terms(),
                component
                    .terms()
                    .iter()
                    .map(|term| term.as_timing_expr().clone())
                    .collect::<Vec<_>>()
            );
        }
        for (path, recipe) in verification.paths().iter().zip(hint.recipes()) {
            assert_eq!(path.steps().len(), recipe.steps.len(), "{}", path.recipe());
            for (actual, declared) in path.steps().iter().zip(&recipe.steps) {
                assert_eq!(actual.kind(), &declared.kind, "{}", path.recipe());
                assert_eq!(actual.operand_index(), declared.operand_index);
                assert_eq!(actual.transition(), declared.transition);
                let expected_order = match &declared.kind {
                    crate::topology_hint::ResolvedPathStepKind::Generated(id) => {
                        transformed
                            .facts
                            .assignments
                            .get(id)
                            .unwrap()
                            .assignment_order
                    }
                    crate::topology_hint::ResolvedPathStepKind::Baseline(id) => transformed
                        .facts
                        .original_assignment_orders
                        .get(&hint.baseline_assignment(id).unwrap().assignment_order())
                        .copied()
                        .unwrap(),
                    crate::topology_hint::ResolvedPathStepKind::Rewrite(id) => {
                        transformed.facts.rewrites.get(id).unwrap().assignment_order
                    }
                };
                assert_eq!(actual.assignment_order(), expected_order);
                assert_eq!(
                    actual.assignment_node(),
                    rebuilt.assignment_id(expected_order).unwrap()
                );
            }
        }
        assert_eq!(
            verification,
            verify_materialized_topology(hint, &transformed, &rebuilt).unwrap()
        );
        let mut corrupt_delay = transformed.clone();
        let reset = corrupt_delay
            .lowered
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "top_q_reset_inv" => {
                    Some(assignment)
                }
                _ => None,
            })
            .unwrap();
        reset.delay = DelayTuple::One(TimingExpr::atom("0").unwrap());
        let changed = reset.clone();
        corrupt_delay
            .facts
            .assignments
            .values_mut()
            .find(|fact| fact.id.as_str() == "q_reset_inv")
            .unwrap()
            .assignment = changed;
        let corrupt_delay_graph = build_timing_graph(
            &corrupt_delay.lowered.cell,
            &corrupt_delay.metadata,
            &corrupt_delay.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let error =
            verify_materialized_topology(hint, &corrupt_delay, &corrupt_delay_graph).unwrap_err();
        assert!(
            error
                .message()
                .contains("actual materialized delay terms do not reconstruct")
        );
        let mut corrupt_ingress = transformed.clone();
        let ingress = corrupt_ingress
            .lowered
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "clk_buf" => {
                    Some(assignment)
                }
                _ => None,
            })
            .unwrap();
        ingress.delay = DelayTuple::One(TimingExpr::atom("ingress_corrupt").unwrap());
        let corrupt_ingress_graph = build_timing_graph(
            &corrupt_ingress.lowered.cell,
            &corrupt_ingress.metadata,
            &corrupt_ingress.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let error = verify_materialized_topology(hint, &corrupt_ingress, &corrupt_ingress_graph)
            .unwrap_err();
        assert!(error.message().contains("baseline ingress"));
        let mut corrupt_q_n_route = transformed.clone();
        let replacement = corrupt_q_n_route
            .lowered
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "top_q_n_replacement" => {
                    Some(assignment)
                }
                _ => None,
            })
            .unwrap();
        replacement.expr = Expr::value(
            ValueOperator::Mux,
            vec![
                Expr::atom("top_all_known"),
                Expr::atom("top_physical_q"),
                Expr::atom("top_q_n_fallback"),
            ],
        );
        let changed = replacement.clone();
        corrupt_q_n_route
            .facts
            .assignments
            .values_mut()
            .find(|fact| fact.id.as_str() == "q_n_replacement")
            .unwrap()
            .assignment = changed;
        let corrupt_q_n_graph = build_timing_graph(
            &corrupt_q_n_route.lowered.cell,
            &corrupt_q_n_route.metadata,
            &corrupt_q_n_route.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let error =
            verify_materialized_topology(hint, &corrupt_q_n_route, &corrupt_q_n_graph).unwrap_err();
        assert!(
            error
                .message()
                .contains("expression does not match resolved topology shape")
        );
        let mut corrupt_guard = transformed.clone();
        let replacement = corrupt_guard
            .lowered
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "top_q_replacement" => {
                    Some(assignment)
                }
                _ => None,
            })
            .unwrap();
        replacement.expr = Expr::value(
            ValueOperator::Mux,
            vec![
                Expr::atom("top_physical_q"),
                Expr::atom("top_physical_q"),
                Expr::atom("top_q_fallback"),
            ],
        );
        let changed = replacement.clone();
        corrupt_guard
            .facts
            .assignments
            .values_mut()
            .find(|fact| fact.id.as_str() == "q_replacement")
            .unwrap()
            .assignment = changed;
        let corrupt_guard_graph = build_timing_graph(
            &corrupt_guard.lowered.cell,
            &corrupt_guard.metadata,
            &corrupt_guard.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let error =
            verify_materialized_topology(hint, &corrupt_guard, &corrupt_guard_graph).unwrap_err();
        assert!(
            error
                .message()
                .contains("expression does not match resolved topology shape")
        );
        let mut corrupt_fact = transformed.clone();
        corrupt_fact
            .facts
            .assignments
            .values_mut()
            .next()
            .unwrap()
            .assignment_order += 1;
        let error = verify_materialized_topology(hint, &corrupt_fact, &rebuilt).unwrap_err();
        assert!(error.message().contains("fact does not match"));
        let mut corrupt_original_orders = transformed.clone();
        corrupt_original_orders
            .facts
            .original_assignment_orders
            .remove(&0);
        let error =
            verify_materialized_topology(hint, &corrupt_original_orders, &rebuilt).unwrap_err();
        assert!(
            error
                .message()
                .contains("original assignment-order map is incomplete")
        );
        let mut corrupt_provenance = transformed.clone();
        let order = corrupt_provenance
            .facts
            .assignments
            .values()
            .next()
            .unwrap()
            .assignment_order;
        let original = corrupt_provenance.provenance[order].clone();
        corrupt_provenance.provenance[order] = AssignmentProvenance::new_with_delay_origin(
            order,
            original.source_assignment_order(),
            original.span().clone(),
            AssignmentOrigin::Source(SourceAssignmentOrigin::Continuous),
            AssignmentDelayOrigin::ImplicitZero,
            original.state_controls().to_vec(),
        )
        .unwrap();
        let error = verify_materialized_topology(hint, &corrupt_provenance, &rebuilt).unwrap_err();
        assert!(error.message().contains("incompatible provenance"));
        let mut corrupt_rewrite = transformed.clone();
        let rewritten_q = corrupt_rewrite
            .lowered
            .cell
            .items
            .iter_mut()
            .find_map(|item| match item {
                CellItem::Assignment(assignment) if assignment.target == "q" => Some(assignment),
                _ => None,
            })
            .unwrap();
        rewritten_q.expr = Expr::atom("top_q_n_replacement");
        let changed = rewritten_q.clone();
        corrupt_rewrite
            .facts
            .rewrites
            .values_mut()
            .find(|fact| fact.baseline.as_str() == "q_state")
            .unwrap()
            .after = changed;
        let corrupt_rewrite_graph = build_timing_graph(
            &corrupt_rewrite.lowered.cell,
            &corrupt_rewrite.metadata,
            &corrupt_rewrite.provenance,
            &baseline.timing_constraint_sources,
        )
        .unwrap();
        let error = verify_materialized_topology(hint, &corrupt_rewrite, &corrupt_rewrite_graph)
            .unwrap_err();
        assert!(
            error
                .message()
                .contains("rewrite fact does not match actual replacement")
        );
        let cut = cut_register_cycles(&rebuilt).unwrap();
        assert_eq!(rebuilt.constraints(), graph.constraints());
        assert!(cut.node_count() > 0);
        let generated_assignment_order = |id: &str| {
            transformed
                .facts
                .assignments
                .values()
                .find(|assignment| assignment.id.as_str() == id)
                .unwrap()
                .assignment_order
        };
        let rewrite_assignment_order = |id: &str| {
            transformed
                .facts
                .rewrites
                .values()
                .find(|rewrite| rewrite.baseline.as_str() == id)
                .unwrap()
                .assignment_order
        };
        let has_operand_edge = |source: &str, target_order: usize, operand_index: usize| {
            let source = rebuilt.signal_id(source).unwrap();
            let target = rebuilt.assignment_id(target_order).unwrap();
            rebuilt.dependencies().iter().any(|dependency| {
                dependency.source() == source
                    && dependency.target() == target
                    && dependency.edge().kind() == crate::timing_graph::DependencyKind::Operand
                    && dependency.edge().operand_index() == Some(operand_index)
            })
        };
        for (source, target, operand_index) in [
            ("clk_buf", "clock_inv", 0),
            ("top_clock_inv", "restored_clock", 0),
            ("top_clock_inv", "fast_mux", 0),
            ("top_restored_clock", "slow_mux", 0),
            ("r_n_buf", "q_reset_inv", 0),
            ("top_q_reset_inv", "master_aoi", 0),
            ("s_n_buf", "master_gate", 1),
            ("top_master_gate", "master_aoi", 1),
            ("top_master_aoi", "fast_mux", 2),
            ("top_master_aoi", "slow_mux", 1),
            ("top_fast_mux", "selector_mux", 1),
            ("top_slow_mux", "selector_mux", 2),
            ("top_selector_mux", "not4", 0),
            ("top_not4", "physical_q", 0),
            ("top_not4", "poststate_gate", 0),
            ("s_n_buf", "poststate_gate", 1),
            ("top_q_n_reset_view", "poststate_aoi", 0),
            ("top_poststate_gate", "poststate_aoi", 1),
            ("top_poststate_aoi", "physical_q_n", 0),
            ("top_physical_q", "q_replacement", 1),
            ("top_physical_q_n", "q_n_replacement", 1),
        ] {
            assert!(
                has_operand_edge(source, generated_assignment_order(target), operand_index),
                "missing generated topology edge {source} -> {target}[{operand_index}]"
            );
        }
        assert!(has_operand_edge(
            "top_q_replacement",
            rewrite_assignment_order("q_state"),
            0
        ));
        assert!(has_operand_edge(
            "top_q_n_replacement",
            rewrite_assignment_order("q_n_output"),
            0
        ));

        let (erased_lowered, erased_provenance, erased_metadata) = transformed
            .erasure
            .erase(
                &transformed.lowered,
                &transformed.provenance,
                &transformed.metadata,
            )
            .unwrap();
        assert_eq!(erased_lowered, baseline.lowered);
        assert_eq!(erased_provenance, baseline.assignment_provenance);
        assert_eq!(erased_metadata, baseline.signal_metadata);
    }

    #[test]
    fn decomposed_delayful_dmg_dffsr_selects_physical_topology() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sv-cells/dmg_cpu_b/cells/dffsr.sv");
        let input = fs::read_to_string(&path).unwrap();
        let design = crate::parser::parse_file(&path, &input).unwrap();
        let first =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap();
        let second =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap();
        assert!(first.is_physical_topology());
        assert!(first.decomposition().is_none());
        assert!(first.strategy().exact_cover().is_none());
        assert_eq!(first.strategy(), second.strategy());
        assert_eq!(first.lowered(), second.lowered());
        assert_eq!(
            first.assignment_provenance(),
            second.assignment_provenance()
        );
        assert_eq!(first.signal_metadata(), second.signal_metadata());
        assert_eq!(
            first.functional_graph().constraints(),
            second.functional_graph().constraints()
        );
        assert_eq!(
            first.cut_graph().dependencies(),
            second.cut_graph().dependencies()
        );
        assert_eq!(
            first.timing_analysis().render(),
            second.timing_analysis().render()
        );
        let (module, facts, verification) = first.strategy().physical_topology().unwrap();
        assert_eq!(module, "dmg_dffsr");
        assert_eq!(facts.assignments.len(), 38);
        assert_eq!(verification.paths().len(), 12);
        assert_eq!(
            verification
                .paths()
                .iter()
                .map(|path| path.recipe())
                .collect::<Vec<_>>(),
            vec![
                "p0_clk_q_rise",
                "p0_clk_q_fall",
                "p1_s_q_rise",
                "p1_s_q_fall",
                "p2_r_q_rise",
                "p2_r_q_fall",
                "p3_clk_q_n_rise",
                "p3_clk_q_n_fall",
                "p4_s_q_n_rise",
                "p4_s_q_n_fall",
                "p5_r_q_n_rise",
                "p5_r_q_n_fall",
            ]
        );
        let source_components = verification
            .paths()
            .iter()
            .map(|path| {
                (
                    path.constraint().ordinal(),
                    path.control().ordinal(),
                    path.target_transition(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(source_components.len(), 12);
        assert_eq!(
            source_components
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            source_components.len()
        );
        assert_eq!(
            source_components
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            (0..6)
                .flat_map(|path| {
                    [Transition::Rise, Transition::Fall]
                        .into_iter()
                        .map(move |transition| (path, path, transition))
                })
                .collect()
        );
        assert_eq!(first.functional_graph().constraints().len(), 6);
        let zero = DelayTuple::One(TimingExpr::atom("0").unwrap());
        let elmore = |length: &str, device, width: &str| {
            let device_width = width.strip_suffix("*2").unwrap_or(width);
            let drive =
                TimingExpr::operation(device, vec![TimingExpr::atom(device_width).unwrap()])
                    .unwrap();
            let drive = if width == "8*2" {
                TimingExpr::operation(
                    TimingOperator::Multiply,
                    vec![drive, TimingExpr::atom("2").unwrap()],
                )
                .unwrap()
            } else {
                drive
            };
            TimingExpr::operation(
                TimingOperator::Elmore,
                vec![
                    TimingExpr::operation(
                        TimingOperator::Wire,
                        vec![TimingExpr::atom(length).unwrap()],
                    )
                    .unwrap(),
                    drive,
                ],
            )
            .unwrap()
        };
        let physical_delays = std::collections::BTreeMap::from([
            (
                "top_q_reset_inv",
                (
                    elmore("L_r_out", TimingOperator::Pmos, "35"),
                    elmore("L_r_out", TimingOperator::Nmos, "35"),
                ),
            ),
            (
                "top_q_n_reset_view",
                (
                    elmore("L_r_out", TimingOperator::Nmos, "35"),
                    elmore("L_r_out", TimingOperator::Pmos, "35"),
                ),
            ),
            (
                "top_clock_inv",
                (
                    elmore("L_clk_n_out", TimingOperator::Pmos, "35"),
                    elmore("L_clk_n_out", TimingOperator::Nmos, "35"),
                ),
            ),
            (
                "top_restored_clock",
                (
                    elmore("L_clk_out", TimingOperator::Pmos, "35"),
                    elmore("L_clk_out", TimingOperator::Nmos, "35"),
                ),
            ),
            (
                "top_master_aoi",
                (
                    elmore("146", TimingOperator::Pmos, "8*2"),
                    elmore("146", TimingOperator::Nmos, "8*2"),
                ),
            ),
            (
                "top_slave_feedback_aoi",
                (
                    elmore("146", TimingOperator::Pmos, "8*2"),
                    elmore("146", TimingOperator::Nmos, "8*2"),
                ),
            ),
            (
                "top_fast_mux",
                (
                    elmore("101", TimingOperator::Pmos, "8"),
                    TimingExpr::atom("0").unwrap(),
                ),
            ),
            (
                "top_slow_mux",
                (
                    TimingExpr::atom("0").unwrap(),
                    elmore("101", TimingOperator::Nmos, "8"),
                ),
            ),
            (
                "top_not4",
                (
                    elmore("104", TimingOperator::Pmos, "8"),
                    elmore("104", TimingOperator::Nmos, "8"),
                ),
            ),
            (
                "top_physical_q",
                (
                    elmore("L_q", TimingOperator::Pmos, "35"),
                    elmore("L_q", TimingOperator::Nmos, "35"),
                ),
            ),
            (
                "top_poststate_aoi",
                (
                    elmore("146", TimingOperator::Pmos, "8*2"),
                    elmore("146", TimingOperator::Nmos, "8*2"),
                ),
            ),
            (
                "top_physical_q_n",
                (
                    elmore("L_q_n", TimingOperator::Pmos, "35"),
                    elmore("L_q_n", TimingOperator::Nmos, "35"),
                ),
            ),
        ]);
        for fact in facts.assignments.values() {
            let expected = physical_delays
                .get(fact.assignment.target.as_str())
                .map(|(rise, fall)| DelayTuple::Two {
                    rise: rise.clone(),
                    fall: fall.clone(),
                })
                .unwrap_or_else(|| zero.clone());
            assert_eq!(fact.assignment.delay, expected, "{}", fact.id);
        }
        assert_eq!(
            facts
                .assignments
                .values()
                .filter(|fact| fact.assignment.delay != zero)
                .count(),
            physical_delays.len()
        );
        assert_eq!(
            first
                .assignment_provenance()
                .iter()
                .filter(|provenance| provenance.origin().is_topology_generated())
                .count(),
            facts.assignments.len()
        );
        let terminal_delays = first
            .lowered()
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment)
                    if assignment.target == "q" || assignment.target == "q_n" =>
                {
                    Some((assignment.target.as_str(), assignment.delay.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal_delays,
            vec![("q", zero.clone()), ("q_n", zero.clone())]
        );
        assert!(
            first
                .lowered()
                .cell
                .items
                .iter()
                .all(|item| matches!(item, CellItem::Assignment(_)))
        );
        let rendered = crate::serialize::render_cell(&first.lowered().cell);
        assert_eq!(
            sexpr_fmt::format_source_default(&rendered).unwrap(),
            rendered
        );
        assert!(!rendered.contains("(timing"));
        assert!(!rendered.contains("(arc"));
        assert!(!rendered.contains("(table"));
        assert_eq!(
            first
                .lowered()
                .cell
                .registers
                .iter()
                .map(|register| (register.name.as_str(), register.initial))
                .collect::<Vec<_>>(),
            vec![("ff", LogicValue::Zero), ("q", LogicValue::Zero)]
        );
        let mut corrupted_lowered = first.lowered().clone();
        corrupted_lowered.cell.name = "corrupt_dffsr".into();
        let error = first
            .erasure()
            .erase(&corrupted_lowered, first.assignment_provenance())
            .unwrap_err();
        assert!(error.message().contains("exact materialized snapshot"));
        let mut corrupted_provenance = first.assignment_provenance().to_vec();
        corrupted_provenance.pop();
        let error = first
            .erasure()
            .erase(first.lowered(), &corrupted_provenance)
            .unwrap_err();
        assert!(
            error
                .message()
                .contains("transformed assignment provenance differs")
        );
        let erased = first
            .erasure()
            .erase(first.lowered(), first.assignment_provenance())
            .unwrap();
        let elaborated = elaborate_design(&design, GenerateMode::Delayful).unwrap();
        let analysis = analyze_design_structural(&elaborated);
        let baseline = lower_elaborated_design_artifacts_with_policy(
            &elaborated,
            &analysis,
            TimingLoweringPolicy::DecompositionBaseline,
        )
        .unwrap();
        assert_eq!(erased.lowered(), &baseline.lowered);
        assert_eq!(
            erased.assignment_provenance(),
            baseline.assignment_provenance
        );
        assert_eq!(erased.signal_metadata(), baseline.signal_metadata);
    }
}
