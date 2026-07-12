use crate::ast::*;
use crate::diagnostic::{Diagnostic, Span};
use crate::elaborate::{GenerateMode, elaborate_design};
use crate::parser::parse_file;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub type AnalyzeResult<T> = Result<T, Diagnostic>;

pub fn analyze_file(path: &Path, input: &str) -> AnalyzeResult<AnalysisReport> {
    analyze_file_with_generate_mode(path, input, GenerateMode::default())
}

pub fn analyze_file_with_generate_mode(
    path: &Path,
    input: &str,
    mode: GenerateMode,
) -> AnalyzeResult<AnalysisReport> {
    let design = parse_file(path, input)?;
    analyze_design_with_generate_mode(&design, mode)
}

/// Performs the M3 structural inventory without selecting generate branches.
pub fn analyze_file_structural(path: &Path, input: &str) -> AnalyzeResult<AnalysisReport> {
    let design = parse_file(path, input)?;
    Ok(analyze_design_structural(&design))
}

pub fn analyze_file_with_catalog(
    path: &Path,
    input: &str,
    catalog: &ModuleCatalog,
) -> AnalyzeResult<AnalysisReport> {
    analyze_file_with_catalog_and_generate_mode(path, input, catalog, GenerateMode::default())
}

pub fn analyze_file_with_catalog_and_generate_mode(
    path: &Path,
    input: &str,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> AnalyzeResult<AnalysisReport> {
    let design = parse_file(path, input)?;
    analyze_design_with_catalog_and_generate_mode(&design, catalog, mode)
}

/// Performs catalog-aware M3 structural inventory without generate selection.
pub fn analyze_file_with_catalog_structural(
    path: &Path,
    input: &str,
    catalog: &ModuleCatalog,
) -> AnalyzeResult<AnalysisReport> {
    let design = parse_file(path, input)?;
    analyze_design_with_catalog_structural(&design, catalog)
}

pub fn analyze_design_with_generate_mode(
    design: &Design,
    mode: GenerateMode,
) -> AnalyzeResult<AnalysisReport> {
    let elaborated = elaborate_design(design, mode)?;
    Ok(analyze_design_structural(&elaborated))
}

/// Performs the M3 structural inventory without selecting generate branches.
pub fn analyze_design_structural(design: &Design) -> AnalysisReport {
    let mut report = AnalysisReport {
        modules: design.modules().map(analyze_module).collect(),
        disposition: AnalysisDisposition::Supported,
        requirements: Vec::new(),
        diagnostics: Vec::new(),
    };
    report.refresh_support_classification();
    report
}

pub fn analyze_design_with_catalog(
    design: &Design,
    catalog: &ModuleCatalog,
) -> AnalyzeResult<AnalysisReport> {
    analyze_design_with_catalog_and_generate_mode(design, catalog, GenerateMode::default())
}

pub fn analyze_design_with_catalog_and_generate_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> AnalyzeResult<AnalysisReport> {
    let elaborated = elaborate_design(design, mode)?;
    analyze_design_with_catalog_structural_mode(&elaborated, catalog, Some(mode))
}

/// Performs catalog-aware M3 structural inventory without generate selection.
pub fn analyze_design_with_catalog_structural(
    design: &Design,
    catalog: &ModuleCatalog,
) -> AnalyzeResult<AnalysisReport> {
    analyze_design_with_catalog_structural_mode(design, catalog, None)
}

fn analyze_design_with_catalog_structural_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<AnalysisReport> {
    let mut report = analyze_design_structural(design);
    for (module, analysis) in design.modules().zip(&mut report.modules) {
        resolve_module_hierarchy(module, analysis, catalog, generate_mode)?;
    }
    report.refresh_support_classification();
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCatalog {
    pub modules: BTreeMap<String, ModuleInterface>,
    definitions: BTreeMap<String, Module>,
}

impl ModuleCatalog {
    pub fn from_designs(designs: &[Design]) -> AnalyzeResult<Self> {
        let mut modules = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for design in designs {
            for module in design.modules() {
                let interface = ModuleInterface::from_module(module);
                if let Some(previous) = modules.insert(module.name.clone(), interface) {
                    return Err(Diagnostic::new(
                        module.span.clone(),
                        format!(
                            "duplicate module `{}`; first declared at {}:{}:{}",
                            module.name,
                            previous.span.path.display(),
                            previous.span.line,
                            previous.span.column
                        ),
                    ));
                }
                definitions.insert(module.name.clone(), module.clone());
            }
        }
        Ok(Self {
            modules,
            definitions,
        })
    }

    pub fn get(&self, name: &str) -> Option<&ModuleInterface> {
        self.modules.get(name)
    }

    /// Returns the typed source definition owned by this catalog.
    ///
    /// Interfaces remain the stable hierarchy-analysis surface; flattening
    /// uses definitions so it can transform the child's typed AST without
    /// reparsing source text.
    pub fn definition(&self, name: &str) -> Option<&Module> {
        self.definitions.get(name)
    }

    fn reaches(
        &self,
        start: &str,
        target: &str,
        generate_mode: Option<GenerateMode>,
    ) -> AnalyzeResult<bool> {
        let mut pending = vec![start.to_string()];
        let mut visited = BTreeSet::new();
        while let Some(name) = pending.pop() {
            if name == target {
                return Ok(true);
            }
            if !visited.insert(name.clone()) {
                continue;
            }
            if let Some(interface) = self.configured_interface(&name, generate_mode)? {
                for reference in interface.references.iter().rev() {
                    if !is_special_instance(&reference.module) {
                        pending.push(reference.module.clone());
                    }
                }
            }
        }
        Ok(false)
    }

    fn configured_interface(
        &self,
        name: &str,
        generate_mode: Option<GenerateMode>,
    ) -> AnalyzeResult<Option<ModuleInterface>> {
        let Some(mode) = generate_mode else {
            return Ok(self.modules.get(name).cloned());
        };
        let Some(definition) = self.definitions.get(name) else {
            return Ok(None);
        };
        let design = Design {
            items: vec![DesignItem::Module(definition.clone())],
        };
        let configured = elaborate_design(&design, mode)?;
        Ok(configured.first_module().map(ModuleInterface::from_module))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInterface {
    pub span: Span,
    pub name: String,
    pub parameters: Vec<InterfaceParameter>,
    pub ports: Vec<InterfacePort>,
    pub references: Vec<ModuleReference>,
}

impl ModuleInterface {
    fn from_module(module: &Module) -> Self {
        let parameters = module
            .parameters
            .iter()
            .map(|parameter| InterfaceParameter {
                span: parameter.span.clone(),
                name: parameter.name.clone(),
                kind: parameter.kind,
                ty: parameter.ty.clone(),
                default: parameter.value.clone(),
            })
            .collect();
        let ports = module
            .ports
            .iter()
            .flat_map(|port| {
                port.names.iter().map(|name| InterfacePort {
                    span: port.span.clone(),
                    name: name.clone(),
                    direction: port.direction,
                    modifiers: port.modifiers.clone(),
                })
            })
            .collect();
        let mut references = Vec::new();
        collect_module_references(&module.items, &mut references);
        Self {
            span: module.span.clone(),
            name: module.name.clone(),
            parameters,
            ports,
            references,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceParameter {
    pub span: Span,
    pub name: String,
    pub kind: ParamKind,
    pub ty: Option<String>,
    pub default: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfacePort {
    pub span: Span,
    pub name: String,
    pub direction: Direction,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleReference {
    pub span: Span,
    pub module: String,
    pub instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisReport {
    pub modules: Vec<ModuleAnalysis>,
    pub disposition: AnalysisDisposition,
    pub requirements: Vec<CapabilityRequirement>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnalysisDisposition {
    Supported,
    Deferred,
    Warned,
    Failed,
}

impl AnalysisDisposition {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Deferred => "deferred",
            Self::Warned => "warned",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetMilestone {
    M3SemanticAnalysis,
    M4FlatCombinational,
    M5StatefulProcedural,
    M6DriversAndStrength,
    M7SymbolicTiming,
    M8GenerateSelection,
    M9OrdinaryHierarchy,
    M10Keeper,
    M11Transistors,
}

impl TargetMilestone {
    pub fn label(self) -> &'static str {
        match self {
            Self::M3SemanticAnalysis => "M3",
            Self::M4FlatCombinational => "M4",
            Self::M5StatefulProcedural => "M5",
            Self::M6DriversAndStrength => "M6",
            Self::M7SymbolicTiming => "M7",
            Self::M8GenerateSelection => "M8",
            Self::M9OrdinaryHierarchy => "M9",
            Self::M10Keeper => "M10",
            Self::M11Transistors => "M11",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub span: Span,
    pub capability_id: String,
    pub milestone: TargetMilestone,
    pub disposition: AnalysisDisposition,
    pub reason: String,
}

impl AnalysisReport {
    pub fn fails(&self, policy: crate::diagnostic::DiagnosticPolicy) -> bool {
        self.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic.kind, crate::diagnostic::DiagnosticKind::Error)
                || (policy.strict
                    && matches!(diagnostic.kind, crate::diagnostic::DiagnosticKind::Warning))
        })
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "analyze summary: modules={} disposition={} requirements={}\n",
            self.modules.len(),
            self.disposition.label(),
            self.requirements.len()
        ));
        for module in &self.modules {
            out.push_str(&module.render());
        }
        if !self.requirements.is_empty() {
            out.push_str("requirements:\n");
            for requirement in &self.requirements {
                out.push_str(&format!(
                    "  {} | {} | {}:{}:{} | {}\n",
                    requirement.capability_id,
                    requirement.milestone.label(),
                    requirement.span.path.display(),
                    requirement.span.line,
                    requirement.span.column,
                    requirement.reason
                ));
            }
        }
        for diagnostic in &self.diagnostics {
            out.push_str(&format!("  {diagnostic}\n"));
        }
        out
    }

    fn refresh_support_classification(&mut self) {
        let mut collector = SupportCollector::default();
        for module in &self.modules {
            classify_module_support(module, &mut collector);
        }
        collector.finish();
        self.disposition = collector
            .requirements
            .iter()
            .map(|requirement| requirement.disposition)
            .max()
            .unwrap_or(AnalysisDisposition::Supported);
        self.requirements = collector.requirements;
        self.diagnostics = collector.diagnostics;
    }
}

#[derive(Default)]
struct SupportCollector {
    requirements: Vec<CapabilityRequirement>,
    deferred_ids: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl SupportCollector {
    fn defer(
        &mut self,
        capability_id: &str,
        milestone: TargetMilestone,
        span: &Span,
        reason: &str,
    ) {
        if self.deferred_ids.insert(capability_id.to_string()) {
            self.requirements.push(CapabilityRequirement {
                span: span.clone(),
                capability_id: capability_id.to_string(),
                milestone,
                disposition: AnalysisDisposition::Deferred,
                reason: reason.to_string(),
            });
        }
    }

    fn fail(&mut self, capability_id: &str, milestone: TargetMilestone, span: &Span, reason: &str) {
        self.requirements.push(CapabilityRequirement {
            span: span.clone(),
            capability_id: capability_id.to_string(),
            milestone,
            disposition: AnalysisDisposition::Failed,
            reason: reason.to_string(),
        });
        self.diagnostics
            .push(Diagnostic::error(span.clone(), reason.to_string()));
    }

    fn finish(&mut self) {
        self.requirements.sort_by(|left, right| {
            left.capability_id
                .cmp(&right.capability_id)
                .then_with(|| left.span.path.cmp(&right.span.path))
                .then_with(|| left.span.line.cmp(&right.span.line))
                .then_with(|| left.span.column.cmp(&right.span.column))
        });
        self.requirements.dedup();
        self.diagnostics.sort_by(|left, right| {
            left.span
                .path
                .cmp(&right.span.path)
                .then_with(|| left.span.line.cmp(&right.span.line))
                .then_with(|| left.span.column.cmp(&right.span.column))
                .then_with(|| left.message.cmp(&right.message))
        });
    }
}

fn classify_module_support(module: &ModuleAnalysis, collector: &mut SupportCollector) {
    for diagnostic in &module.semantic_diagnostics {
        collector.fail(
            "invalid.symbol.duplicate",
            TargetMilestone::M3SemanticAnalysis,
            &diagnostic.span,
            &diagnostic.message,
        );
    }
    classify_support_parts(
        &module.continuous_assignments,
        &module.initial_assignments,
        &module.procedural_assignments,
        &module.primitive_calls,
        &module.instantiations,
        &module.timing_aliases,
        &module.specify_paths,
        &module.drivers,
        &module.generate_alternatives,
        collector,
    );
}

fn classify_scope_support(scope: &ScopeAnalysis, collector: &mut SupportCollector) {
    for diagnostic in &scope.semantic_diagnostics {
        collector.fail(
            "invalid.symbol.duplicate",
            TargetMilestone::M3SemanticAnalysis,
            &diagnostic.span,
            &diagnostic.message,
        );
    }
    classify_support_parts(
        &scope.continuous_assignments,
        &scope.initial_assignments,
        &scope.procedural_assignments,
        &scope.primitive_calls,
        &scope.instantiations,
        &scope.timing_aliases,
        &scope.specify_paths,
        &scope.drivers,
        &scope.generate_alternatives,
        collector,
    );
}

#[allow(clippy::too_many_arguments)]
fn classify_support_parts(
    continuous_assignments: &[AssignmentAnalysis],
    initial_assignments: &[AssignmentAnalysis],
    procedural_assignments: &[AssignmentAnalysis],
    primitive_calls: &[PrimitiveAnalysis],
    instantiations: &[InstantiationAnalysis],
    timing_aliases: &BTreeMap<String, TimingAlias>,
    specify_paths: &[SpecPathAnalysis],
    drivers: &[DriverAnalysis],
    generate_alternatives: &[GenerateAlternativeAnalysis],
    collector: &mut SupportCollector,
) {
    if let Some(assignment) = continuous_assignments.first() {
        collector.defer(
            "value.combinational",
            TargetMilestone::M4FlatCombinational,
            &assignment.span,
            "combinational value lowering is scheduled for Milestone 4",
        );
    }
    if let Some(assignment) = procedural_assignments.iter().find(|assignment| {
        matches!(
            assignment.kind,
            AssignmentKind::Procedural { state: false, .. }
        )
    }) {
        collector.defer(
            "value.procedural-combinational",
            TargetMilestone::M4FlatCombinational,
            &assignment.span,
            "combinational procedural lowering is scheduled for Milestone 4",
        );
    }

    for initial in initial_assignments {
        let target_is_scalar = expr_local_symbol(&initial.target_expression).is_some();
        let value_is_literal = is_contracted_literal(&initial.value_expression);
        if !target_is_scalar {
            collector.fail(
                "invalid.initial.target",
                TargetMilestone::M5StatefulProcedural,
                &initial.target_expression.span,
                "initial assignment target must be a scalar local signal",
            );
        }
        if !value_is_literal {
            collector.fail(
                "invalid.initial.value",
                TargetMilestone::M5StatefulProcedural,
                &initial.value_expression.span,
                "initial assignment value must be a contracted literal (0, 1, '0, '1, 'x, or 'z)",
            );
        }
        if target_is_scalar && value_is_literal {
            collector.defer(
                "state.initial",
                TargetMilestone::M5StatefulProcedural,
                &initial.span,
                "literal initialization and modeled state are scheduled for Milestone 5",
            );
        }
    }
    if let Some(assignment) = procedural_assignments.iter().find(|assignment| {
        matches!(
            assignment.kind,
            AssignmentKind::Procedural { state: true, .. }
        )
    }) {
        collector.defer(
            "state.procedural",
            TargetMilestone::M5StatefulProcedural,
            &assignment.span,
            "stateful procedural lowering is scheduled for Milestone 5",
        );
    }

    if let Some(assignment) = continuous_assignments
        .iter()
        .find(|assignment| assignment.strength.is_some())
    {
        collector.defer(
            "driver.strength",
            TargetMilestone::M6DriversAndStrength,
            &assignment.span,
            "strength-qualified drivers are scheduled for Milestone 6",
        );
    }
    if let Some(assignment) = continuous_assignments
        .iter()
        .find(|assignment| expr_contains_constant(&assignment.value_expression, ConstKind::Z))
    {
        collector.defer(
            "driver.high-z",
            TargetMilestone::M6DriversAndStrength,
            &assignment.value_expression.span,
            "high-impedance driver semantics are scheduled for Milestone 6",
        );
    }
    if let Some(primitive) = primitive_calls.iter().find(|primitive| {
        matches!(primitive.name.as_str(), "bufif0" | "bufif1") || primitive.strength.is_some()
    }) {
        collector.defer(
            "driver.primitive-tristate",
            TargetMilestone::M6DriversAndStrength,
            &primitive.span,
            "tri-state primitive and strength semantics are scheduled for Milestone 6",
        );
    }
    let mut driver_counts = BTreeMap::<&str, usize>::new();
    for driver in drivers {
        *driver_counts.entry(&driver.target).or_default() += 1;
    }
    if let Some(driver) = drivers
        .iter()
        .find(|driver| driver_counts[driver.target.as_str()] > 1)
    {
        collector.defer(
            "driver.repeated",
            TargetMilestone::M6DriversAndStrength,
            &driver.span,
            "repeated source drivers are scheduled for Milestone 6",
        );
    }

    if let Some(assignment) = continuous_assignments
        .iter()
        .find(|assignment| !assignment.delay_expressions.is_empty())
    {
        collector.defer(
            "timing.assignment-delay",
            TargetMilestone::M7SymbolicTiming,
            &assignment.span,
            "symbolic assignment delays are scheduled for Milestone 7",
        );
    }
    if let Some(primitive) = primitive_calls
        .iter()
        .find(|primitive| primitive.delay.is_some())
    {
        collector.defer(
            "timing.primitive-delay",
            TargetMilestone::M7SymbolicTiming,
            &primitive.span,
            "symbolic primitive delays are scheduled for Milestone 7",
        );
    }
    if let Some(alias) = timing_aliases.values().next() {
        collector.defer(
            "timing.alias",
            TargetMilestone::M7SymbolicTiming,
            &alias.span,
            "timing aliases are scheduled for Milestone 7",
        );
    }
    if let Some(path) = specify_paths.first() {
        collector.defer(
            "timing.specify-path",
            TargetMilestone::M7SymbolicTiming,
            &path.span,
            "specify paths are scheduled for Milestone 7",
        );
    }

    if let Some(alternative) = generate_alternatives.first() {
        collector.defer(
            "generate.alternative",
            TargetMilestone::M8GenerateSelection,
            &alternative.span,
            "generate branch selection is scheduled for Milestone 8",
        );
    }
    for alternative in generate_alternatives {
        classify_scope_support(&alternative.then_branch, collector);
        if let Some(else_branch) = &alternative.else_branch {
            classify_scope_support(else_branch, collector);
        }
    }

    for instantiation in instantiations {
        match &instantiation.resolution {
            InstantiationResolution::Resolved(_) => {}
            InstantiationResolution::Special(_) => {}
            _ if instantiation.module == "keeper" => {
                collector.defer(
                    "hierarchy.keeper",
                    TargetMilestone::M10Keeper,
                    &instantiation.span,
                    "keeper behavior is scheduled for Milestone 10",
                );
            }
            InstantiationResolution::Unresolved => collector.defer(
                "hierarchy.ordinary",
                TargetMilestone::M9OrdinaryHierarchy,
                &instantiation.span,
                "ordinary hierarchy lowering is scheduled for Milestone 9",
            ),
        }
    }

    if let Some(primitive) = primitive_calls
        .iter()
        .find(|primitive| matches!(primitive.name.as_str(), "nmos" | "pmos" | "rnmos"))
    {
        collector.defer(
            "primitive.transistor",
            TargetMilestone::M11Transistors,
            &primitive.span,
            "direct transistor primitives are scheduled for Milestone 11",
        );
    }
}

fn expr_contains_constant(expr: &Expr, expected: ConstKind) -> bool {
    match &expr.kind {
        ExprKind::Constant(actual) => *actual == expected,
        ExprKind::Group(inner) | ExprKind::Unary { expr: inner, .. } => {
            expr_contains_constant(inner, expected)
        }
        ExprKind::Binary { left, right, .. } => {
            expr_contains_constant(left, expected) || expr_contains_constant(right, expected)
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => {
            expr_contains_constant(condition, expected)
                || expr_contains_constant(then_expr, expected)
                || expr_contains_constant(else_expr, expected)
        }
        ExprKind::Call { callee, args } => {
            expr_contains_constant(callee, expected)
                || args
                    .iter()
                    .flatten()
                    .any(|argument| expr_contains_constant(argument, expected))
        }
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAnalysis {
    pub span: Span,
    pub name: String,
    pub symbols: BTreeMap<String, SymbolAnalysis>,
    pub ports: BTreeMap<String, PortAnalysis>,
    pub parameters: BTreeMap<String, ValueAnalysis>,
    pub declarations: BTreeMap<String, DeclAnalysis>,
    pub localparams: BTreeMap<String, ValueAnalysis>,
    pub specparams: BTreeMap<String, ValueAnalysis>,
    pub timing_aliases: BTreeMap<String, TimingAlias>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub registers: Vec<String>,
    pub continuous_assignments: Vec<AssignmentAnalysis>,
    pub initial_assignments: Vec<AssignmentAnalysis>,
    pub procedural_assignments: Vec<AssignmentAnalysis>,
    pub primitive_calls: Vec<PrimitiveAnalysis>,
    pub instantiations: Vec<InstantiationAnalysis>,
    pub specify_paths: Vec<SpecPathAnalysis>,
    pub signal_roles: BTreeMap<String, SignalAnalysis>,
    pub drivers: Vec<DriverAnalysis>,
    pub generate_alternatives: Vec<GenerateAlternativeAnalysis>,
    pub semantic_diagnostics: Vec<Diagnostic>,
}

impl ModuleAnalysis {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "module {} @{}\n",
            self.name,
            render_span(&self.span)
        ));
        out.push_str(&format!("  inputs: {}\n", join_list(&self.inputs)));
        out.push_str(&format!("  outputs: {}\n", join_list(&self.outputs)));
        out.push_str(&format!("  registers: {}\n", join_list(&self.registers)));
        render_symbols(&mut out, 2, &self.symbols);
        push_indented(&mut out, 2, "ports:");
        for (name, port) in &self.ports {
            push_indented(
                &mut out,
                4,
                &format!(
                    "{} direction={} use={} modifiers=[{}] @{}",
                    name,
                    direction_label(port.direction),
                    port_use_label(port.is_input, port.is_output),
                    port.modifiers.join(","),
                    render_span(&port.span)
                ),
            );
        }
        render_declarations(&mut out, 2, &self.declarations);
        render_semantic_contents(
            &mut out,
            2,
            &self.signal_roles,
            &self.continuous_assignments,
            &self.initial_assignments,
            &self.procedural_assignments,
            &self.drivers,
            &self.primitive_calls,
            &self.instantiations,
            &self.timing_aliases,
            &self.specify_paths,
            &self.generate_alternatives,
        );
        out
    }
}

fn render_span(span: &Span) -> String {
    format!("{}:{}:{}", span.path.display(), span.line, span.column)
}

fn port_use_label(is_input: bool, is_output: bool) -> &'static str {
    match (is_input, is_output) {
        (true, true) => "input+output",
        (true, false) => "input",
        (false, true) => "output",
        (false, false) => "unused",
    }
}

fn push_indented(out: &mut String, indent: usize, line: &str) {
    out.push_str(&" ".repeat(indent));
    out.push_str(line);
    out.push('\n');
}

fn render_symbols(out: &mut String, indent: usize, symbols: &BTreeMap<String, SymbolAnalysis>) {
    push_indented(out, indent, "symbols:");
    for (name, symbol) in symbols {
        push_indented(
            out,
            indent + 2,
            &format!(
                "{} category={} @{}",
                name,
                symbol_category_label(symbol.category),
                render_span(&symbol.span)
            ),
        );
    }
}

fn render_declarations(
    out: &mut String,
    indent: usize,
    declarations: &BTreeMap<String, DeclAnalysis>,
) {
    push_indented(out, indent, "declarations:");
    for (name, declaration) in declarations {
        push_indented(
            out,
            indent + 2,
            &format!(
                "{} kind={:?} type={} value={} @{}",
                name,
                declaration.kind,
                declaration.ty.as_deref().unwrap_or("<implicit>"),
                declaration.value.as_deref().unwrap_or("<none>"),
                render_span(&declaration.span)
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_semantic_contents(
    out: &mut String,
    indent: usize,
    signal_roles: &BTreeMap<String, SignalAnalysis>,
    continuous_assignments: &[AssignmentAnalysis],
    initial_assignments: &[AssignmentAnalysis],
    procedural_assignments: &[AssignmentAnalysis],
    drivers: &[DriverAnalysis],
    primitive_calls: &[PrimitiveAnalysis],
    instantiations: &[InstantiationAnalysis],
    timing_aliases: &BTreeMap<String, TimingAlias>,
    specify_paths: &[SpecPathAnalysis],
    generate_alternatives: &[GenerateAlternativeAnalysis],
) {
    push_indented(out, indent, "signal_roles:");
    for (name, signal) in signal_roles {
        let roles = signal
            .roles
            .iter()
            .map(|role| format!("{role:?}"))
            .collect::<Vec<_>>()
            .join(",");
        push_indented(
            out,
            indent + 2,
            &format!(
                "{} roles=[{}] declared={}",
                name,
                roles,
                signal
                    .declaration_span
                    .as_ref()
                    .map(render_span)
                    .unwrap_or_else(|| "<implicit>".to_string())
            ),
        );
    }
    push_indented(out, indent, "assignments:");
    for assignment in continuous_assignments
        .iter()
        .chain(initial_assignments)
        .chain(procedural_assignments)
    {
        push_indented(
            out,
            indent + 2,
            &format!(
                "#{} {} @{} strength={} syntax={:?}",
                assignment.source_order,
                assignment.render(),
                render_span(&assignment.span),
                assignment
                    .strength
                    .as_ref()
                    .map(|strength| strength.values.join(","))
                    .unwrap_or_else(|| "<none>".to_string()),
                assignment.kind
            ),
        );
    }
    push_indented(out, indent, "drivers:");
    for driver in drivers {
        push_indented(
            out,
            indent + 2,
            &format!(
                "#{} target={} source={} @{}",
                driver.source_order,
                driver.target,
                render_driver_source(&driver.source),
                render_span(&driver.span)
            ),
        );
    }
    push_indented(out, indent, "primitives:");
    for primitive in primitive_calls {
        push_indented(
            out,
            indent + 2,
            &format!(
                "#{} {} args=[{}] strength={} delay={} @{}",
                primitive.source_order,
                primitive.name,
                primitive
                    .args
                    .iter()
                    .map(|argument| argument.as_deref().unwrap_or("_"))
                    .collect::<Vec<_>>()
                    .join(","),
                primitive
                    .strength
                    .as_ref()
                    .map(|strength| strength.join(","))
                    .unwrap_or_else(|| "<none>".to_string()),
                primitive.delay.as_deref().unwrap_or("<none>"),
                render_span(&primitive.span)
            ),
        );
    }
    push_indented(out, indent, "instantiations:");
    for instantiation in instantiations {
        render_instantiation(out, indent + 2, instantiation);
    }
    push_indented(out, indent, "timing_aliases:");
    for (name, alias) in timing_aliases {
        push_indented(
            out,
            indent + 2,
            &format!(
                "{} kind={:?} value={} @{}",
                name,
                alias.kind,
                alias.value,
                render_span(&alias.span)
            ),
        );
    }
    push_indented(out, indent, "specify_paths:");
    for path in specify_paths {
        push_indented(
            out,
            indent + 2,
            &format!(
                "controls=[{}] target={} delays=[{}] @{}",
                path.controls
                    .iter()
                    .map(|control| control.text.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                path.target.text,
                path.delays
                    .iter()
                    .map(|delay| delay.as_ref().map(|expr| expr.text.as_str()).unwrap_or("_"))
                    .collect::<Vec<_>>()
                    .join(","),
                render_span(&path.span)
            ),
        );
    }
    push_indented(out, indent, "generate_alternatives:");
    for (index, alternative) in generate_alternatives.iter().enumerate() {
        push_indented(
            out,
            indent + 2,
            &format!(
                "[{}] condition={} @{}",
                index,
                alternative.condition.text,
                render_span(&alternative.span)
            ),
        );
        render_scope(out, indent + 4, "then", &alternative.then_branch);
        if let Some(else_branch) = &alternative.else_branch {
            render_scope(out, indent + 4, "else", else_branch);
        }
    }
}

fn render_scope(out: &mut String, indent: usize, label: &str, scope: &ScopeAnalysis) {
    push_indented(
        out,
        indent,
        &format!("{} scope @{}", label, render_span(&scope.span)),
    );
    render_symbols(out, indent + 2, &scope.symbols);
    render_declarations(out, indent + 2, &scope.declarations);
    push_indented(
        out,
        indent + 2,
        &format!("registers: {}", join_list(&scope.registers)),
    );
    render_semantic_contents(
        out,
        indent + 2,
        &scope.signal_roles,
        &scope.continuous_assignments,
        &scope.initial_assignments,
        &scope.procedural_assignments,
        &scope.drivers,
        &scope.primitive_calls,
        &scope.instantiations,
        &scope.timing_aliases,
        &scope.specify_paths,
        &scope.generate_alternatives,
    );
}

fn render_driver_source(source: &DriverSource) -> String {
    match source {
        DriverSource::Continuous => "continuous".to_string(),
        DriverSource::Initial => "initial".to_string(),
        DriverSource::Procedural { state, source } => {
            format!("procedural(state={state},source={source:?})")
        }
        DriverSource::Primitive { name } => format!("primitive({name})"),
        DriverSource::Keeper { instance } => format!("keeper(instance={instance})"),
        DriverSource::Hierarchical {
            module,
            instance,
            port,
        } => format!(
            "hierarchical(module={module},instance={instance},port={})",
            port.as_deref().unwrap_or("<unknown>")
        ),
    }
}

fn render_instantiation(out: &mut String, indent: usize, instantiation: &InstantiationAnalysis) {
    push_indented(
        out,
        indent,
        &format!(
            "#{} {} {} @{}",
            instantiation.source_order,
            instantiation.module,
            instantiation.instance,
            render_span(&instantiation.span)
        ),
    );
    match &instantiation.resolution {
        InstantiationResolution::Resolved(resolved) => {
            push_indented(out, indent + 2, "resolution=resolved");
            for binding in &resolved.parameter_bindings {
                push_indented(
                    out,
                    indent + 2,
                    &format!(
                        "parameter {} source={:?} value={} @{}",
                        binding.parameter,
                        binding.source,
                        render_expr(&binding.expression),
                        render_span(&binding.span)
                    ),
                );
            }
            for connection in &resolved.connections {
                push_indented(
                    out,
                    indent + 2,
                    &format!(
                        "connection {} direction={} source={:?} value={} local={} @{}",
                        connection.port,
                        direction_label(connection.direction),
                        connection.source,
                        render_expr(&connection.expression),
                        connection.local_signal.as_deref().unwrap_or("<expression>"),
                        render_span(&connection.span)
                    ),
                );
            }
        }
        InstantiationResolution::Special(special) => {
            push_indented(
                out,
                indent + 2,
                &format!("resolution=special({:?})", special.kind),
            );
            push_indented(
                out,
                indent + 2,
                &format!(
                    "connection target={} @{}",
                    special.keeper.connection.target,
                    render_span(&special.keeper.connection.span)
                ),
            );
        }
        InstantiationResolution::Unresolved => {
            push_indented(out, indent + 2, "resolution=unresolved");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAnalysis {
    pub span: Span,
    pub direction: Direction,
    pub modifiers: Vec<String>,
    pub declared: String,
    pub is_input: bool,
    pub is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclAnalysis {
    pub span: Span,
    pub kind: DeclKind,
    pub ty: Option<String>,
    pub value: Option<String>,
    pub expression: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueAnalysis {
    pub span: Span,
    pub kind: ParamKind,
    pub ty: Option<String>,
    pub value: String,
    pub expression: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingAlias {
    pub span: Span,
    pub kind: ParamKind,
    pub value: String,
    pub expression: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentAnalysis {
    pub span: Span,
    pub source_order: usize,
    pub target: String,
    pub value: String,
    pub delay: Option<String>,
    pub target_expression: Expr,
    pub value_expression: Expr,
    pub delay_expressions: Vec<Option<Expr>>,
    pub strength: Option<Strength>,
    pub kind: AssignmentKind,
}

impl AssignmentAnalysis {
    fn render(&self) -> String {
        match &self.delay {
            Some(delay) => format!(
                "({} {} {} {})",
                self.target,
                self.kind.label(),
                self.value,
                delay
            ),
            None => format!("({} {} {})", self.target, self.kind.label(), self.value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentKind {
    Continuous,
    Initial,
    Procedural {
        state: bool,
        source: ProceduralSource,
    },
}

impl AssignmentKind {
    fn label(&self) -> &'static str {
        match self {
            AssignmentKind::Continuous => "continuous",
            AssignmentKind::Initial => "initial",
            AssignmentKind::Procedural { state: true, .. } => "state",
            AssignmentKind::Procedural { state: false, .. } => "procedural",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProceduralSource {
    AlwaysLatch,
    AlwaysFf,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimitiveAnalysis {
    pub span: Span,
    pub source_order: usize,
    pub name: String,
    pub strength: Option<Vec<String>>,
    pub delay: Option<String>,
    pub args: Vec<Option<String>>,
    pub argument_expressions: Vec<Option<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstantiationAnalysis {
    pub span: Span,
    pub source_order: usize,
    pub module: String,
    pub instance: String,
    pub parameters: Vec<String>,
    pub connections: Vec<String>,
    pub parameter_overrides: Vec<ParamOverride>,
    pub connection_items: Vec<Connection>,
    pub resolution: InstantiationResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstantiationResolution {
    Unresolved,
    Resolved(ResolvedInstantiation),
    Special(SpecialInstantiation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInstantiation {
    pub interface_span: Span,
    pub parameter_bindings: Vec<ResolvedParameterBinding>,
    pub connections: Vec<ResolvedConnection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedParameterBinding {
    pub span: Span,
    pub parameter: String,
    pub source: ParameterBindingSource,
    pub expression: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterBindingSource {
    Named,
    Positional { index: usize },
    OmittedPositional { index: usize },
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConnection {
    pub span: Span,
    pub port: String,
    pub direction: Direction,
    pub source: ConnectionSource,
    pub expression: Expr,
    pub local_signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionSource {
    Named,
    Positional { index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialInstantiation {
    pub kind: SpecialInstanceKind,
    pub keeper: KeeperInstantiation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialInstanceKind {
    Keeper,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperInstantiation {
    pub span: Span,
    pub instance: String,
    pub connection: KeeperConnection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperConnection {
    pub span: Span,
    pub target: String,
    pub expression: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecPathAnalysis {
    pub span: Span,
    pub controls: Vec<ExpressionAnalysis>,
    pub target: ExpressionAnalysis,
    pub delays: Vec<Option<ExpressionAnalysis>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionAnalysis {
    pub span: Span,
    pub text: String,
    pub expression: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolCategory {
    Port,
    Parameter,
    Declaration,
    Localparam,
    Specparam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolAnalysis {
    pub span: Span,
    pub category: SymbolCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignalRole {
    ContinuousDriven,
    ProceduralDriven,
    ModeledState,
    PrimitiveDriven,
    KeeperDriven,
    HierarchicalConnection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalAnalysis {
    pub declaration_span: Option<Span>,
    pub roles: BTreeSet<SignalRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverAnalysis {
    pub span: Span,
    pub source_order: usize,
    pub target: String,
    pub source: DriverSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverSource {
    Continuous,
    Initial,
    Procedural {
        state: bool,
        source: ProceduralSource,
    },
    Primitive {
        name: String,
    },
    Keeper {
        instance: String,
    },
    Hierarchical {
        module: String,
        instance: String,
        port: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateAlternativeAnalysis {
    pub span: Span,
    pub condition: ExpressionAnalysis,
    pub then_branch: ScopeAnalysis,
    pub else_branch: Option<ScopeAnalysis>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeAnalysis {
    pub span: Span,
    pub symbols: BTreeMap<String, SymbolAnalysis>,
    pub parameters: BTreeMap<String, ValueAnalysis>,
    pub declarations: BTreeMap<String, DeclAnalysis>,
    pub localparams: BTreeMap<String, ValueAnalysis>,
    pub specparams: BTreeMap<String, ValueAnalysis>,
    pub timing_aliases: BTreeMap<String, TimingAlias>,
    pub registers: Vec<String>,
    pub continuous_assignments: Vec<AssignmentAnalysis>,
    pub initial_assignments: Vec<AssignmentAnalysis>,
    pub procedural_assignments: Vec<AssignmentAnalysis>,
    pub primitive_calls: Vec<PrimitiveAnalysis>,
    pub instantiations: Vec<InstantiationAnalysis>,
    pub specify_paths: Vec<SpecPathAnalysis>,
    pub signal_roles: BTreeMap<String, SignalAnalysis>,
    pub drivers: Vec<DriverAnalysis>,
    pub generate_alternatives: Vec<GenerateAlternativeAnalysis>,
    pub semantic_diagnostics: Vec<Diagnostic>,
}

impl ScopeAnalysis {
    fn new(span: Span) -> Self {
        Self {
            span,
            symbols: BTreeMap::new(),
            parameters: BTreeMap::new(),
            declarations: BTreeMap::new(),
            localparams: BTreeMap::new(),
            specparams: BTreeMap::new(),
            timing_aliases: BTreeMap::new(),
            registers: Vec::new(),
            continuous_assignments: Vec::new(),
            initial_assignments: Vec::new(),
            procedural_assignments: Vec::new(),
            primitive_calls: Vec::new(),
            instantiations: Vec::new(),
            specify_paths: Vec::new(),
            signal_roles: BTreeMap::new(),
            drivers: Vec::new(),
            generate_alternatives: Vec::new(),
            semantic_diagnostics: Vec::new(),
        }
    }
}

fn collect_module_references(items: &[Item], references: &mut Vec<ModuleReference>) {
    for item in items {
        match &item.kind {
            ItemKind::Instantiation(instantiation) => references.push(ModuleReference {
                span: instantiation.span.clone(),
                module: instantiation.module.clone(),
                instance: instantiation.instance.clone(),
            }),
            ItemKind::AlwaysLatch(always) => {
                collect_module_references(std::slice::from_ref(always.body.as_ref()), references);
            }
            ItemKind::Always(always) => {
                collect_module_references(std::slice::from_ref(always.body.as_ref()), references);
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                collect_module_references(&block.items, references);
            }
            ItemKind::If(statement) => {
                collect_module_references(
                    std::slice::from_ref(statement.then_branch.as_ref()),
                    references,
                );
                if let Some(else_branch) = &statement.else_branch {
                    collect_module_references(
                        std::slice::from_ref(else_branch.as_ref()),
                        references,
                    );
                }
            }
            ItemKind::Import(_)
            | ItemKind::Decl(_)
            | ItemKind::Initial(_)
            | ItemKind::ProcAssign(_)
            | ItemKind::Assign(_)
            | ItemKind::Primitive(_)
            | ItemKind::Specify(_)
            | ItemKind::Empty => {}
        }
    }
}

pub(crate) fn is_special_instance(module: &str) -> bool {
    module == "keeper"
}

/// Validates and resolves the contracted scalar `keeper instance(target)` form.
///
/// This typed bridge is shared by catalog-aware analysis and keeper lowering;
/// callers never need to reconstruct recognized syntax from rendered text.
pub(crate) fn resolve_keeper_ast_instantiation(
    instantiation: &Instantiation,
    visible_signals: &BTreeMap<String, Span>,
) -> AnalyzeResult<KeeperInstantiation> {
    resolve_keeper_instantiation(
        &instantiation.span,
        &instantiation.module,
        &instantiation.instance,
        &instantiation.parameters,
        &instantiation.connections,
        visible_signals,
    )
}

fn resolve_keeper_instantiation(
    span: &Span,
    module: &str,
    instance: &str,
    parameters: &[ParamOverride],
    connections: &[Connection],
    visible_signals: &BTreeMap<String, Span>,
) -> AnalyzeResult<KeeperInstantiation> {
    debug_assert_eq!(module, "keeper");
    if let Some(parameter) = parameters.first() {
        return Err(Diagnostic::new(
            parameter.span.clone(),
            format!("keeper instance `{instance}` does not accept parameter overrides"),
        ));
    }
    let connection = match connections {
        [] => {
            return Err(Diagnostic::new(
                span.clone(),
                format!("keeper instance `{instance}` requires exactly one positional connection"),
            ));
        }
        [connection] => connection,
        [_, extra, ..] => {
            return Err(Diagnostic::new(
                extra.span.clone(),
                format!("keeper instance `{instance}` requires exactly one positional connection"),
            ));
        }
    };
    let expression = match &connection.kind {
        ConnectionKind::Named { .. } => {
            return Err(Diagnostic::new(
                connection.span.clone(),
                format!("keeper instance `{instance}` requires a positional connection"),
            ));
        }
        ConnectionKind::Positional(expression) => expression,
    };
    let ExprKind::Path(segments) = &expression.kind else {
        return Err(Diagnostic::new(
            expression.span.clone(),
            format!("keeper instance `{instance}` target must be a scalar signal name"),
        ));
    };
    let [target] = segments.as_slice() else {
        return Err(Diagnostic::new(
            expression.span.clone(),
            format!("keeper instance `{instance}` target must be a scalar signal name"),
        ));
    };
    if !visible_signals.contains_key(target) {
        return Err(Diagnostic::new(
            expression.span.clone(),
            format!("unknown keeper target `{target}` for instance `{instance}`"),
        ));
    }
    Ok(KeeperInstantiation {
        span: span.clone(),
        instance: instance.to_string(),
        connection: KeeperConnection {
            span: connection.span.clone(),
            target: target.clone(),
            expression: expression.clone(),
        },
    })
}

/// Resolves one typed AST instantiation using the same binding rules exposed
/// by catalog-aware analysis.
pub(crate) fn resolve_ast_instantiation(
    current_module: &str,
    instantiation: &Instantiation,
    visible_signals: &BTreeMap<String, Span>,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<InstantiationResolution> {
    if is_special_instance(&instantiation.module) {
        return Ok(InstantiationResolution::Special(SpecialInstantiation {
            kind: SpecialInstanceKind::Keeper,
            keeper: resolve_keeper_ast_instantiation(instantiation, visible_signals)?,
        }));
    }
    let analysis = InstantiationAnalysis {
        span: instantiation.span.clone(),
        source_order: 0,
        module: instantiation.module.clone(),
        instance: instantiation.instance.clone(),
        // These rendered report-only fields are not consulted by resolution.
        // Keep the hierarchy bridge entirely typed rather than manufacturing
        // text for an AST that will immediately be transformed again.
        parameters: Vec::new(),
        connections: Vec::new(),
        parameter_overrides: instantiation.parameters.clone(),
        connection_items: instantiation.connections.clone(),
        resolution: InstantiationResolution::Unresolved,
    };
    resolve_instantiation(
        current_module,
        &analysis,
        visible_signals,
        catalog,
        generate_mode,
    )
}

fn resolve_module_hierarchy(
    module: &Module,
    analysis: &mut ModuleAnalysis,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<()> {
    let mut visible_signals = analysis
        .declarations
        .iter()
        .map(|(name, declaration)| (name.clone(), declaration.span.clone()))
        .collect::<BTreeMap<_, _>>();
    visible_signals.extend(
        analysis
            .ports
            .iter()
            .map(|(name, port)| (name.clone(), port.span.clone())),
    );
    let mut port_usage = PortUsage {
        inputs: analysis.inputs.clone(),
        outputs: analysis.outputs.clone(),
    };
    resolve_instantiation_list(
        &module.name,
        &mut analysis.instantiations,
        &mut analysis.signal_roles,
        &visible_signals,
        &mut analysis.drivers,
        &mut analysis.ports,
        &mut port_usage,
        catalog,
        generate_mode,
    )?;
    for alternative in &mut analysis.generate_alternatives {
        resolve_scope_hierarchy(
            &module.name,
            &mut alternative.then_branch,
            &mut analysis.ports,
            &mut port_usage,
            &visible_signals,
            catalog,
            generate_mode,
        )?;
        if let Some(else_branch) = &mut alternative.else_branch {
            resolve_scope_hierarchy(
                &module.name,
                else_branch,
                &mut analysis.ports,
                &mut port_usage,
                &visible_signals,
                catalog,
                generate_mode,
            )?;
        }
    }
    analysis.inputs = port_usage.inputs;
    analysis.outputs = port_usage.outputs;
    Ok(())
}

fn resolve_scope_hierarchy(
    current_module: &str,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    parent_visible_signals: &BTreeMap<String, Span>,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<()> {
    let mut visible_signals = parent_visible_signals.clone();
    visible_signals.extend(
        scope
            .declarations
            .iter()
            .map(|(name, declaration)| (name.clone(), declaration.span.clone())),
    );
    resolve_instantiation_list(
        current_module,
        &mut scope.instantiations,
        &mut scope.signal_roles,
        &visible_signals,
        &mut scope.drivers,
        ports,
        port_usage,
        catalog,
        generate_mode,
    )?;
    for alternative in &mut scope.generate_alternatives {
        resolve_scope_hierarchy(
            current_module,
            &mut alternative.then_branch,
            ports,
            port_usage,
            &visible_signals,
            catalog,
            generate_mode,
        )?;
        if let Some(else_branch) = &mut alternative.else_branch {
            resolve_scope_hierarchy(
                current_module,
                else_branch,
                ports,
                port_usage,
                &visible_signals,
                catalog,
                generate_mode,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolve_instantiation_list(
    current_module: &str,
    instantiations: &mut [InstantiationAnalysis],
    signal_roles: &mut BTreeMap<String, SignalAnalysis>,
    visible_signals: &BTreeMap<String, Span>,
    drivers: &mut Vec<DriverAnalysis>,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<()> {
    for instantiation in instantiations {
        let resolution = resolve_instantiation(
            current_module,
            instantiation,
            visible_signals,
            catalog,
            generate_mode,
        )?;
        match &resolution {
            InstantiationResolution::Resolved(resolved) => {
                for connection in &resolved.connections {
                    if let Some(name) = &connection.local_signal {
                        let signal =
                            signal_roles
                                .entry(name.clone())
                                .or_insert_with(|| SignalAnalysis {
                                    declaration_span: visible_signals.get(name).cloned(),
                                    roles: BTreeSet::new(),
                                });
                        if signal.declaration_span.is_none() {
                            signal.declaration_span = visible_signals.get(name).cloned();
                        }
                        signal.roles.insert(SignalRole::HierarchicalConnection);
                    }
                    match connection.direction {
                        Direction::Input => {
                            collect_behavioral_reads(&connection.expression, ports, port_usage);
                        }
                        Direction::Output => {
                            apply_hierarchical_output(
                                instantiation,
                                connection,
                                drivers,
                                ports,
                                port_usage,
                            );
                        }
                        Direction::Inout => {
                            collect_behavioral_reads(&connection.expression, ports, port_usage);
                            apply_hierarchical_output(
                                instantiation,
                                connection,
                                drivers,
                                ports,
                                port_usage,
                            );
                        }
                    }
                }
            }
            InstantiationResolution::Special(special) => apply_keeper_resolution(
                instantiation,
                &special.keeper,
                signal_roles,
                visible_signals,
                drivers,
                ports,
                port_usage,
            ),
            InstantiationResolution::Unresolved => {}
        }
        instantiation.resolution = resolution;
    }
    drivers.sort_by_key(|driver| driver.source_order);
    Ok(())
}

fn apply_keeper_resolution(
    instantiation: &InstantiationAnalysis,
    keeper: &KeeperInstantiation,
    signal_roles: &mut BTreeMap<String, SignalAnalysis>,
    visible_signals: &BTreeMap<String, Span>,
    drivers: &mut Vec<DriverAnalysis>,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    let connection = &keeper.connection;
    let signal = signal_roles
        .entry(connection.target.clone())
        .or_insert_with(|| SignalAnalysis {
            declaration_span: visible_signals.get(&connection.target).cloned(),
            roles: BTreeSet::new(),
        });
    if signal.declaration_span.is_none() {
        signal.declaration_span = visible_signals.get(&connection.target).cloned();
    }
    signal.roles.insert(SignalRole::KeeperDriven);
    mark_port_write(&connection.expression, ports, port_usage);
    drivers.push(DriverAnalysis {
        span: connection.span.clone(),
        source_order: instantiation.source_order,
        target: connection.target.clone(),
        source: DriverSource::Keeper {
            instance: keeper.instance.clone(),
        },
    });
}

fn apply_hierarchical_output(
    instantiation: &InstantiationAnalysis,
    connection: &ResolvedConnection,
    drivers: &mut Vec<DriverAnalysis>,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    mark_port_write(&connection.expression, ports, port_usage);
    drivers.push(DriverAnalysis {
        span: connection.span.clone(),
        source_order: instantiation.source_order,
        target: render_expr(&connection.expression),
        source: DriverSource::Hierarchical {
            module: instantiation.module.clone(),
            instance: instantiation.instance.clone(),
            port: Some(connection.port.clone()),
        },
    });
}

fn resolve_instantiation(
    current_module: &str,
    instantiation: &InstantiationAnalysis,
    visible_signals: &BTreeMap<String, Span>,
    catalog: &ModuleCatalog,
    generate_mode: Option<GenerateMode>,
) -> AnalyzeResult<InstantiationResolution> {
    if is_special_instance(&instantiation.module) {
        let keeper = resolve_keeper_instantiation(
            &instantiation.span,
            &instantiation.module,
            &instantiation.instance,
            &instantiation.parameter_overrides,
            &instantiation.connection_items,
            visible_signals,
        )?;
        return Ok(InstantiationResolution::Special(SpecialInstantiation {
            kind: SpecialInstanceKind::Keeper,
            keeper,
        }));
    }
    let interface = catalog.get(&instantiation.module).ok_or_else(|| {
        Diagnostic::new(
            instantiation.span.clone(),
            format!(
                "unknown instantiated module `{}` for instance `{}`",
                instantiation.module, instantiation.instance
            ),
        )
    })?;
    if instantiation.module == current_module
        || catalog.reaches(&instantiation.module, current_module, generate_mode)?
    {
        return Err(Diagnostic::new(
            instantiation.span.clone(),
            format!(
                "recursive module reference from `{}` through `{}`",
                current_module, instantiation.module
            ),
        ));
    }
    let parameter_bindings = resolve_parameter_bindings(instantiation, interface)?;
    let connections = resolve_connections(instantiation, interface, visible_signals)?;
    Ok(InstantiationResolution::Resolved(ResolvedInstantiation {
        interface_span: interface.span.clone(),
        parameter_bindings,
        connections,
    }))
}

fn resolve_parameter_bindings(
    instantiation: &InstantiationAnalysis,
    interface: &ModuleInterface,
) -> AnalyzeResult<Vec<ResolvedParameterBinding>> {
    let has_named = instantiation
        .parameter_overrides
        .iter()
        .any(|item| matches!(item.kind, ParamOverrideKind::Named { .. }));
    let has_positional = instantiation
        .parameter_overrides
        .iter()
        .any(|item| matches!(item.kind, ParamOverrideKind::Positional(_)));
    if has_named && has_positional {
        let offending = instantiation
            .parameter_overrides
            .iter()
            .skip(1)
            .find(|item| {
                matches!(item.kind, ParamOverrideKind::Named { .. })
                    != matches!(
                        instantiation.parameter_overrides[0].kind,
                        ParamOverrideKind::Named { .. }
                    )
            })
            .unwrap_or(&instantiation.parameter_overrides[0]);
        return Err(Diagnostic::new(
            offending.span.clone(),
            "cannot mix named and positional parameter overrides",
        ));
    }

    let mut bindings = Vec::new();
    let mut bound = BTreeSet::new();
    if has_named {
        for override_item in &instantiation.parameter_overrides {
            let ParamOverrideKind::Named { name, value } = &override_item.kind else {
                unreachable!("mixed parameter forms rejected above")
            };
            let parameter = interface
                .parameters
                .iter()
                .find(|parameter| parameter.name == *name)
                .ok_or_else(|| {
                    Diagnostic::new(
                        override_item.span.clone(),
                        format!("unknown parameter `{name}` on module `{}`", interface.name),
                    )
                })?;
            if !bound.insert(name.clone()) {
                return Err(Diagnostic::new(
                    override_item.span.clone(),
                    format!("duplicate parameter override `{name}`"),
                ));
            }
            bindings.push(ResolvedParameterBinding {
                span: override_item.span.clone(),
                parameter: name.clone(),
                source: ParameterBindingSource::Named,
                expression: value.clone(),
            });
            debug_assert_eq!(parameter.name, *name);
        }
    } else {
        if instantiation.parameter_overrides.len() > interface.parameters.len() {
            let excess = &instantiation.parameter_overrides[interface.parameters.len()];
            return Err(Diagnostic::new(
                excess.span.clone(),
                format!(
                    "too many positional parameter overrides for module `{}`: expected at most {}",
                    interface.name,
                    interface.parameters.len()
                ),
            ));
        }
        for (index, override_item) in instantiation.parameter_overrides.iter().enumerate() {
            let ParamOverrideKind::Positional(value) = &override_item.kind else {
                unreachable!("named parameter forms handled above")
            };
            let parameter = &interface.parameters[index];
            let (source, expression) = match value {
                Some(value) => (ParameterBindingSource::Positional { index }, value.clone()),
                None => (
                    ParameterBindingSource::OmittedPositional { index },
                    parameter.default.clone(),
                ),
            };
            bound.insert(parameter.name.clone());
            bindings.push(ResolvedParameterBinding {
                span: override_item.span.clone(),
                parameter: parameter.name.clone(),
                source,
                expression,
            });
        }
    }
    for parameter in &interface.parameters {
        if !bound.contains(&parameter.name) {
            bindings.push(ResolvedParameterBinding {
                span: parameter.span.clone(),
                parameter: parameter.name.clone(),
                source: ParameterBindingSource::Default,
                expression: parameter.default.clone(),
            });
        }
    }
    Ok(bindings)
}

fn resolve_connections(
    instantiation: &InstantiationAnalysis,
    interface: &ModuleInterface,
    visible_signals: &BTreeMap<String, Span>,
) -> AnalyzeResult<Vec<ResolvedConnection>> {
    let has_named = instantiation
        .connection_items
        .iter()
        .any(|item| matches!(item.kind, ConnectionKind::Named { .. }));
    let has_positional = instantiation
        .connection_items
        .iter()
        .any(|item| matches!(item.kind, ConnectionKind::Positional(_)));
    if has_named && has_positional {
        let first_is_named = matches!(
            instantiation.connection_items[0].kind,
            ConnectionKind::Named { .. }
        );
        let offending = instantiation
            .connection_items
            .iter()
            .skip(1)
            .find(|item| matches!(item.kind, ConnectionKind::Named { .. }) != first_is_named)
            .unwrap_or(&instantiation.connection_items[0]);
        return Err(Diagnostic::new(
            offending.span.clone(),
            "cannot mix named and positional port connections",
        ));
    }

    let mut connections = Vec::new();
    if has_named {
        let mut connected = BTreeSet::new();
        for connection in &instantiation.connection_items {
            let ConnectionKind::Named { name, value } = &connection.kind else {
                unreachable!("mixed connection forms rejected above")
            };
            let port = interface
                .ports
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| {
                    Diagnostic::new(
                        connection.span.clone(),
                        format!("unknown port `{name}` on module `{}`", interface.name),
                    )
                })?;
            if !connected.insert(name.clone()) {
                return Err(Diagnostic::new(
                    connection.span.clone(),
                    format!("duplicate port connection `{name}`"),
                ));
            }
            connections.push(resolved_connection(
                connection,
                port,
                ConnectionSource::Named,
                value,
                visible_signals,
            )?);
        }
        if let Some(missing) = interface
            .ports
            .iter()
            .find(|port| !connected.contains(&port.name))
        {
            return Err(Diagnostic::new(
                instantiation.span.clone(),
                format!("missing connection for port `{}`", missing.name),
            ));
        }
    } else {
        if instantiation.connection_items.len() != interface.ports.len() {
            let span = instantiation
                .connection_items
                .get(interface.ports.len())
                .map(|connection| connection.span.clone())
                .unwrap_or_else(|| instantiation.span.clone());
            return Err(Diagnostic::new(
                span,
                format!(
                    "wrong positional port arity for module `{}`: expected {}, found {}",
                    interface.name,
                    interface.ports.len(),
                    instantiation.connection_items.len()
                ),
            ));
        }
        for (index, (connection, port)) in instantiation
            .connection_items
            .iter()
            .zip(&interface.ports)
            .enumerate()
        {
            let ConnectionKind::Positional(value) = &connection.kind else {
                unreachable!("named connection forms handled above")
            };
            connections.push(resolved_connection(
                connection,
                port,
                ConnectionSource::Positional { index },
                value,
                visible_signals,
            )?);
        }
    }
    Ok(connections)
}

fn resolved_connection(
    connection: &Connection,
    port: &InterfacePort,
    source: ConnectionSource,
    value: &Expr,
    visible_signals: &BTreeMap<String, Span>,
) -> AnalyzeResult<ResolvedConnection> {
    let local_signal = expr_local_symbol(value).map(str::to_string);
    if matches!(port.direction, Direction::Output | Direction::Inout) && local_signal.is_none() {
        return Err(Diagnostic::new(
            connection.span.clone(),
            format!(
                "connection to {} port `{}` must be an assignable scalar local signal",
                direction_label(port.direction),
                port.name
            ),
        ));
    }
    if matches!(port.direction, Direction::Output | Direction::Inout)
        && local_signal
            .as_ref()
            .is_some_and(|name| !visible_signals.contains_key(name))
    {
        return Err(Diagnostic::new(
            connection.span.clone(),
            format!(
                "connection to {} port `{}` references undeclared local signal `{}`",
                direction_label(port.direction),
                port.name,
                local_signal.as_deref().unwrap_or_default()
            ),
        ));
    }
    Ok(ResolvedConnection {
        span: connection.span.clone(),
        port: port.name.clone(),
        direction: port.direction,
        source,
        expression: value.clone(),
        local_signal,
    })
}

fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Input => "input",
        Direction::Output => "output",
        Direction::Inout => "inout",
    }
}

fn analyze_module(module: &Module) -> ModuleAnalysis {
    let mut scope = ScopeAnalysis::new(module.span.clone());
    let mut ports = BTreeMap::new();
    let mut port_usage = PortUsage::default();
    let mut source_order = 0;

    for param in &module.parameters {
        let value = render_expr(&param.value);
        let entry = ValueAnalysis {
            span: param.span.clone(),
            kind: param.kind,
            ty: param.ty.clone(),
            value: value.clone(),
            expression: param.value.clone(),
        };
        match param.kind {
            ParamKind::Parameter => {
                if insert_symbol(
                    &mut scope,
                    &param.name,
                    param.span.clone(),
                    SymbolCategory::Parameter,
                ) {
                    scope.parameters.insert(param.name.clone(), entry);
                }
            }
            ParamKind::Localparam => {
                if insert_symbol(
                    &mut scope,
                    &param.name,
                    param.span.clone(),
                    SymbolCategory::Localparam,
                ) {
                    scope.localparams.insert(param.name.clone(), entry.clone());
                    scope.timing_aliases.insert(
                        param.name.clone(),
                        TimingAlias {
                            span: param.span.clone(),
                            kind: param.kind,
                            value,
                            expression: param.value.clone(),
                        },
                    );
                }
            }
            ParamKind::Specparam => {
                if insert_symbol(
                    &mut scope,
                    &param.name,
                    param.span.clone(),
                    SymbolCategory::Specparam,
                ) {
                    scope.specparams.insert(param.name.clone(), entry.clone());
                    scope.timing_aliases.insert(
                        param.name.clone(),
                        TimingAlias {
                            span: param.span.clone(),
                            kind: param.kind,
                            value,
                            expression: param.value.clone(),
                        },
                    );
                }
            }
        }
    }

    for port in &module.ports {
        for name in &port.names {
            if insert_symbol(&mut scope, name, port.span.clone(), SymbolCategory::Port) {
                ports.insert(
                    name.clone(),
                    PortAnalysis {
                        span: port.span.clone(),
                        direction: port.direction,
                        modifiers: port.modifiers.clone(),
                        declared: name.clone(),
                        is_input: matches!(port.direction, Direction::Input),
                        is_output: matches!(port.direction, Direction::Output),
                    },
                );
                match port.direction {
                    Direction::Input => push_unique(&mut port_usage.inputs, name.clone()),
                    Direction::Output => push_unique(&mut port_usage.outputs, name.clone()),
                    Direction::Inout => {}
                }
            }
        }
    }

    for item in &module.items {
        analyze_item(
            item,
            &mut scope,
            &mut ports,
            &mut port_usage,
            &mut source_order,
            ProceduralContext::Root,
        );
    }

    ModuleAnalysis {
        span: module.span.clone(),
        name: module.name.clone(),
        symbols: scope.symbols,
        ports,
        parameters: scope.parameters,
        declarations: scope.declarations,
        localparams: scope.localparams,
        specparams: scope.specparams,
        timing_aliases: scope.timing_aliases,
        inputs: port_usage.inputs,
        outputs: port_usage.outputs,
        registers: scope.registers,
        continuous_assignments: scope.continuous_assignments,
        initial_assignments: scope.initial_assignments,
        procedural_assignments: scope.procedural_assignments,
        primitive_calls: scope.primitive_calls,
        instantiations: scope.instantiations,
        specify_paths: scope.specify_paths,
        signal_roles: scope.signal_roles,
        drivers: scope.drivers,
        generate_alternatives: scope.generate_alternatives,
        semantic_diagnostics: scope.semantic_diagnostics,
    }
}

#[derive(Default)]
struct PortUsage {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum ProceduralContext {
    Root,
    Always {
        state: bool,
        source: ProceduralSource,
    },
}

impl ProceduralContext {
    fn is_state(self) -> bool {
        matches!(self, ProceduralContext::Always { state: true, .. })
    }

    fn source(self) -> Option<ProceduralSource> {
        match self {
            ProceduralContext::Root => None,
            ProceduralContext::Always { source, .. } => Some(source),
        }
    }
}

fn analyze_item(
    item: &Item,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: &mut usize,
    context: ProceduralContext,
) {
    match &item.kind {
        ItemKind::Import(_) | ItemKind::Empty => {}
        ItemKind::Decl(decl) => analyze_decl(decl, scope),
        ItemKind::Initial(stmt) => analyze_initial(
            stmt,
            scope,
            ports,
            port_usage,
            next_source_order(source_order),
        ),
        ItemKind::ProcAssign(stmt) => analyze_assignment(
            stmt,
            scope,
            ports,
            port_usage,
            context,
            next_source_order(source_order),
        ),
        ItemKind::AlwaysLatch(always) => {
            if let Some(condition) = &always.condition {
                collect_behavioral_reads(condition, ports, port_usage);
            }
            analyze_item(
                &always.body,
                scope,
                ports,
                port_usage,
                source_order,
                ProceduralContext::Always {
                    state: true,
                    source: ProceduralSource::AlwaysLatch,
                },
            );
        }
        ItemKind::Always(always) => {
            if let Some(sensitivity) = &always.sensitivity {
                collect_sensitivity_reads(sensitivity, ports, port_usage);
            }
            if always_is_stateful(always) {
                analyze_item(
                    &always.body,
                    scope,
                    ports,
                    port_usage,
                    source_order,
                    ProceduralContext::Always {
                        state: true,
                        source: match always.kind {
                            AlwaysKind::Ff => ProceduralSource::AlwaysFf,
                            _ => ProceduralSource::Always,
                        },
                    },
                );
                return;
            }
            analyze_item(
                &always.body,
                scope,
                ports,
                port_usage,
                source_order,
                ProceduralContext::Always {
                    state: false,
                    source: ProceduralSource::Always,
                },
            );
        }
        ItemKind::Assign(assign) => analyze_continuous_assign(
            assign,
            scope,
            ports,
            port_usage,
            next_source_order(source_order),
        ),
        ItemKind::Primitive(call) => analyze_primitive(
            call,
            scope,
            ports,
            port_usage,
            next_source_order(source_order),
        ),
        ItemKind::Instantiation(instantiation) => analyze_instantiation(
            instantiation,
            scope,
            ports,
            port_usage,
            next_source_order(source_order),
        ),
        ItemKind::Specify(specify) => analyze_specify(specify, scope),
        ItemKind::Generate(block) => {
            analyze_generate_block(block, scope, ports, port_usage, source_order, context)
        }
        ItemKind::Block(block) => {
            for child in &block.items {
                analyze_item(child, scope, ports, port_usage, source_order, context);
            }
        }
        ItemKind::If(stmt) => {
            collect_behavioral_reads(&stmt.condition, ports, port_usage);
            analyze_item(
                &stmt.then_branch,
                scope,
                ports,
                port_usage,
                source_order,
                context,
            );
            if let Some(else_branch) = &stmt.else_branch {
                analyze_item(else_branch, scope, ports, port_usage, source_order, context);
            }
        }
    }
}

fn analyze_generate_block(
    block: &Block,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: &mut usize,
    context: ProceduralContext,
) {
    for item in &block.items {
        if let ItemKind::If(stmt) = &item.kind {
            analyze_generate_alternative(stmt, scope, ports, port_usage, source_order, context);
        } else {
            analyze_item(item, scope, ports, port_usage, source_order, context);
        }
    }
}

fn analyze_generate_alternative(
    stmt: &IfStmt,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: &mut usize,
    context: ProceduralContext,
) {
    collect_behavioral_reads(&stmt.condition, ports, port_usage);
    let mut then_branch = ScopeAnalysis::new(stmt.then_branch.span.clone());
    analyze_item(
        &stmt.then_branch,
        &mut then_branch,
        ports,
        port_usage,
        source_order,
        context,
    );
    let else_branch = stmt.else_branch.as_ref().map(|branch| {
        let mut branch_scope = ScopeAnalysis::new(branch.span.clone());
        analyze_item(
            branch,
            &mut branch_scope,
            ports,
            port_usage,
            source_order,
            context,
        );
        branch_scope
    });
    scope
        .generate_alternatives
        .push(GenerateAlternativeAnalysis {
            span: stmt.span.clone(),
            condition: expression_analysis(&stmt.condition),
            then_branch,
            else_branch,
        });
}

fn analyze_decl(decl: &Decl, scope: &mut ScopeAnalysis) {
    let value = decl.value.as_ref().map(render_expr);
    match decl.kind {
        DeclKind::Logic | DeclKind::Tri | DeclKind::Wire => {
            for name in &decl.names {
                if insert_symbol(scope, name, decl.span.clone(), SymbolCategory::Declaration) {
                    scope.declarations.insert(
                        name.clone(),
                        DeclAnalysis {
                            span: decl.span.clone(),
                            kind: decl.kind,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                            expression: decl.value.clone(),
                        },
                    );
                    scope
                        .signal_roles
                        .entry(name.clone())
                        .or_insert_with(|| SignalAnalysis {
                            declaration_span: Some(decl.span.clone()),
                            roles: BTreeSet::new(),
                        });
                }
            }
        }
        DeclKind::Parameter => {
            for name in &decl.names {
                if let (Some(value), Some(expression)) = (&value, &decl.value)
                    && insert_symbol(scope, name, decl.span.clone(), SymbolCategory::Parameter)
                {
                    scope.parameters.insert(
                        name.clone(),
                        ValueAnalysis {
                            span: decl.span.clone(),
                            kind: ParamKind::Parameter,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                            expression: expression.clone(),
                        },
                    );
                }
            }
        }
        DeclKind::Localparam => {
            for name in &decl.names {
                if let (Some(value), Some(expression)) = (&value, &decl.value)
                    && insert_symbol(scope, name, decl.span.clone(), SymbolCategory::Localparam)
                {
                    scope.localparams.insert(
                        name.clone(),
                        ValueAnalysis {
                            span: decl.span.clone(),
                            kind: ParamKind::Localparam,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                            expression: expression.clone(),
                        },
                    );
                    scope.timing_aliases.insert(
                        name.clone(),
                        TimingAlias {
                            span: decl.span.clone(),
                            kind: ParamKind::Localparam,
                            value: value.clone(),
                            expression: expression.clone(),
                        },
                    );
                }
            }
        }
        DeclKind::Specparam => {
            for name in &decl.names {
                if let (Some(value), Some(expression)) = (&value, &decl.value)
                    && insert_symbol(scope, name, decl.span.clone(), SymbolCategory::Specparam)
                {
                    scope.specparams.insert(
                        name.clone(),
                        ValueAnalysis {
                            span: decl.span.clone(),
                            kind: ParamKind::Specparam,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                            expression: expression.clone(),
                        },
                    );
                    scope.timing_aliases.insert(
                        name.clone(),
                        TimingAlias {
                            span: decl.span.clone(),
                            kind: ParamKind::Specparam,
                            value: value.clone(),
                            expression: expression.clone(),
                        },
                    );
                }
            }
        }
    }
}

fn analyze_initial(
    stmt: &AssignStmt,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: usize,
) {
    let target = render_expr(&stmt.target);
    let value = render_expr(&stmt.value);
    collect_behavioral_reads(&stmt.value, ports, port_usage);
    mark_port_write(&stmt.target, ports, port_usage);
    if is_contracted_literal(&stmt.value)
        && let Some(name) = expr_local_symbol(&stmt.target)
    {
        register_name(name, &mut scope.registers);
        add_signal_role(scope, name, SignalRole::ModeledState, ports);
    }
    record_driver(
        scope,
        &stmt.target,
        stmt.span.clone(),
        source_order,
        DriverSource::Initial,
    );
    scope.initial_assignments.push(AssignmentAnalysis {
        span: stmt.span.clone(),
        source_order,
        target,
        value,
        delay: None,
        target_expression: stmt.target.clone(),
        value_expression: stmt.value.clone(),
        delay_expressions: Vec::new(),
        strength: None,
        kind: AssignmentKind::Initial,
    });
}

fn analyze_assignment(
    stmt: &AssignStmt,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    context: ProceduralContext,
    source_order: usize,
) {
    let target = render_expr(&stmt.target);
    let value = render_expr(&stmt.value);
    collect_behavioral_reads(&stmt.value, ports, port_usage);
    mark_port_write(&stmt.target, ports, port_usage);
    let kind = AssignmentKind::Procedural {
        state: context.is_state(),
        source: context.source().unwrap_or(ProceduralSource::Always),
    };
    scope.procedural_assignments.push(AssignmentAnalysis {
        span: stmt.span.clone(),
        source_order,
        target,
        value,
        delay: None,
        target_expression: stmt.target.clone(),
        value_expression: stmt.value.clone(),
        delay_expressions: Vec::new(),
        strength: None,
        kind,
    });
    if context.is_state()
        && let Some(name) = expr_local_symbol(&stmt.target)
    {
        register_name(name, &mut scope.registers);
        add_signal_role(scope, name, SignalRole::ModeledState, ports);
    } else if let Some(name) = expr_local_symbol(&stmt.target) {
        add_signal_role(scope, name, SignalRole::ProceduralDriven, ports);
    }
    record_driver(
        scope,
        &stmt.target,
        stmt.span.clone(),
        source_order,
        DriverSource::Procedural {
            state: context.is_state(),
            source: context.source().unwrap_or(ProceduralSource::Always),
        },
    );
}

fn analyze_continuous_assign(
    assign: &AssignDecl,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: usize,
) {
    let target = render_expr(&assign.target);
    let value = render_expr(&assign.value);
    collect_behavioral_reads(&assign.value, ports, port_usage);
    mark_port_write(&assign.target, ports, port_usage);
    if let Some(name) = expr_local_symbol(&assign.target) {
        add_signal_role(scope, name, SignalRole::ContinuousDriven, ports);
    }
    record_driver(
        scope,
        &assign.target,
        assign.span.clone(),
        source_order,
        DriverSource::Continuous,
    );
    scope.continuous_assignments.push(AssignmentAnalysis {
        span: assign.span.clone(),
        source_order,
        target,
        value,
        delay: assign.delay.as_ref().map(render_delay),
        target_expression: assign.target.clone(),
        value_expression: assign.value.clone(),
        delay_expressions: assign
            .delay
            .as_ref()
            .map(|delay| delay.values.clone())
            .unwrap_or_default(),
        strength: assign.strength.clone(),
        kind: AssignmentKind::Continuous,
    });
}

fn analyze_primitive(
    call: &PrimitiveCall,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: usize,
) {
    if let Some(strength) = &call.strength {
        // Strength groups are preserved as raw identifiers for deterministic summaries.
        let _ = strength;
    }
    let mut args = Vec::with_capacity(call.args.len());
    for (index, arg) in call.args.iter().enumerate() {
        match arg {
            Some(expr) => {
                if index == 0 {
                    mark_port_write(expr, ports, port_usage);
                    if let Some(name) = expr_local_symbol(expr) {
                        add_signal_role(scope, name, SignalRole::PrimitiveDriven, ports);
                    }
                    record_driver(
                        scope,
                        expr,
                        call.span.clone(),
                        source_order,
                        DriverSource::Primitive {
                            name: call.name.clone(),
                        },
                    );
                } else {
                    collect_behavioral_reads(expr, ports, port_usage);
                }
                args.push(Some(render_expr(expr)));
            }
            None => args.push(None),
        }
    }
    scope.primitive_calls.push(PrimitiveAnalysis {
        span: call.span.clone(),
        source_order,
        name: call.name.clone(),
        strength: call.strength.as_ref().map(|s| s.values.clone()),
        delay: call.delay.as_ref().map(render_delay),
        args,
        argument_expressions: call.args.clone(),
    });
}

fn analyze_instantiation(
    inst: &Instantiation,
    scope: &mut ScopeAnalysis,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
    source_order: usize,
) {
    let mut parameters = Vec::with_capacity(inst.parameters.len());
    for override_item in &inst.parameters {
        match &override_item.kind {
            ParamOverrideKind::Named { value, .. } => {
                parameters.push(render_expr(value));
            }
            ParamOverrideKind::Positional(Some(value)) => {
                parameters.push(render_expr(value));
            }
            ParamOverrideKind::Positional(None) => parameters.push("_".to_string()),
        }
    }
    let mut connections = Vec::with_capacity(inst.connections.len());
    for connection in &inst.connections {
        match &connection.kind {
            ConnectionKind::Named { value, .. } => {
                if !is_special_instance(&inst.module) {
                    mark_hierarchical_connection(value, scope, ports);
                }
                connections.push(render_expr(value));
            }
            ConnectionKind::Positional(value) => {
                if !is_special_instance(&inst.module) {
                    mark_hierarchical_connection(value, scope, ports);
                }
                connections.push(render_expr(value));
            }
        }
    }
    // Connection directions are deliberately not guessed here. Phase 2 resolves
    // them against a module catalog before marking behavioral reads or writes.
    let _ = port_usage;
    scope.instantiations.push(InstantiationAnalysis {
        span: inst.span.clone(),
        source_order,
        module: inst.module.clone(),
        instance: inst.instance.clone(),
        parameters,
        connections,
        parameter_overrides: inst.parameters.clone(),
        connection_items: inst.connections.clone(),
        resolution: InstantiationResolution::Unresolved,
    });
}

fn analyze_specify(specify: &SpecifyBlock, scope: &mut ScopeAnalysis) {
    for item in &specify.items {
        match item {
            SpecifyItem::Specparam(param) => {
                analyze_decl(
                    &Decl {
                        span: param.span.clone(),
                        kind: DeclKind::Specparam,
                        ty: param.ty.clone(),
                        names: vec![param.name.clone()],
                        value: Some(param.value.clone()),
                    },
                    scope,
                );
            }
            SpecifyItem::Path(path) => {
                scope.specify_paths.push(SpecPathAnalysis {
                    span: path.span.clone(),
                    controls: path.controls.iter().map(expression_analysis).collect(),
                    target: expression_analysis(&path.target),
                    delays: path
                        .delays
                        .iter()
                        .map(|item| item.as_ref().map(expression_analysis))
                        .collect(),
                });
            }
        }
    }
}

pub(crate) fn sensitivity_is_stateful(sensitivity: &Sensitivity, kind: AlwaysKind) -> bool {
    match kind {
        AlwaysKind::Ff => true,
        AlwaysKind::Comb => false,
        AlwaysKind::Plain => match &sensitivity.kind {
            SensitivityKind::Any => false,
            SensitivityKind::List(list) => list.iter().any(|item| item.edge.is_some()),
        },
    }
}

fn always_is_stateful(always: &AlwaysBlock) -> bool {
    match always.kind {
        AlwaysKind::Ff => true,
        AlwaysKind::Comb => false,
        AlwaysKind::Plain => always
            .sensitivity
            .as_ref()
            .is_some_and(|sensitivity| sensitivity_is_stateful(sensitivity, always.kind)),
    }
}

fn is_contracted_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Constant(ConstKind::Zero | ConstKind::One | ConstKind::X | ConstKind::Z) => true,
        ExprKind::Integer(value) if matches!(value.as_str(), "0" | "1") => true,
        ExprKind::Group(inner) => is_contracted_literal(inner),
        _ => false,
    }
}

fn collect_behavioral_reads(
    expr: &Expr,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    match &expr.kind {
        ExprKind::Path(segments) => {
            if let [name] = segments.as_slice() {
                mark_port_read(name, ports, port_usage);
            }
        }
        ExprKind::Group(inner) => collect_behavioral_reads(inner, ports, port_usage),
        ExprKind::Unary { expr, .. } => collect_behavioral_reads(expr, ports, port_usage),
        ExprKind::Binary { left, right, .. } => {
            collect_behavioral_reads(left, ports, port_usage);
            collect_behavioral_reads(right, ports, port_usage);
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_behavioral_reads(condition, ports, port_usage);
            collect_behavioral_reads(then_expr, ports, port_usage);
            collect_behavioral_reads(else_expr, ports, port_usage);
        }
        ExprKind::Call { callee, args } => {
            collect_behavioral_reads(callee, ports, port_usage);
            for expr in args.iter().flatten() {
                collect_behavioral_reads(expr, ports, port_usage);
            }
        }
        ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {}
    }
}

fn collect_sensitivity_reads(
    sensitivity: &Sensitivity,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    if let SensitivityKind::List(events) = &sensitivity.kind {
        for expr in events.iter().filter_map(|event| event.expr.as_ref()) {
            collect_behavioral_reads(expr, ports, port_usage);
        }
    }
}

fn mark_port_write(
    expr: &Expr,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    if let Some(name) = expr_local_symbol(expr)
        && let Some(port) = ports.get_mut(name)
        && matches!(port.direction, Direction::Inout)
    {
        port.is_output = true;
        push_unique(&mut port_usage.outputs, name.to_string());
    }
}

fn mark_port_read(
    name: &str,
    ports: &mut BTreeMap<String, PortAnalysis>,
    port_usage: &mut PortUsage,
) {
    if let Some(port) = ports.get_mut(name)
        && matches!(port.direction, Direction::Inout)
    {
        port.is_input = true;
        push_unique(&mut port_usage.inputs, name.to_string());
    }
}

fn register_name(name: &str, registers: &mut Vec<String>) {
    push_unique(registers, name.to_string());
}

fn expr_local_symbol(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Path(segments) if segments.len() == 1 => segments.first().map(String::as_str),
        ExprKind::Group(inner) => expr_local_symbol(inner),
        _ => None,
    }
}

fn expression_analysis(expr: &Expr) -> ExpressionAnalysis {
    ExpressionAnalysis {
        span: expr.span.clone(),
        text: render_expr(expr),
        expression: expr.clone(),
    }
}

fn insert_symbol(
    scope: &mut ScopeAnalysis,
    name: &str,
    span: Span,
    category: SymbolCategory,
) -> bool {
    if let Some(previous) = scope.symbols.get(name) {
        scope.semantic_diagnostics.push(Diagnostic::error(
            span,
            format!(
                "duplicate symbol `{name}` as {}; first declared as {} at {}:{}:{}",
                symbol_category_label(category),
                symbol_category_label(previous.category),
                previous.span.path.display(),
                previous.span.line,
                previous.span.column
            ),
        ));
        false
    } else {
        scope
            .symbols
            .insert(name.to_string(), SymbolAnalysis { span, category });
        true
    }
}

fn symbol_category_label(category: SymbolCategory) -> &'static str {
    match category {
        SymbolCategory::Port => "port",
        SymbolCategory::Parameter => "parameter",
        SymbolCategory::Declaration => "declaration",
        SymbolCategory::Localparam => "localparam",
        SymbolCategory::Specparam => "specparam",
    }
}

fn add_signal_role(
    scope: &mut ScopeAnalysis,
    name: &str,
    role: SignalRole,
    ports: &BTreeMap<String, PortAnalysis>,
) {
    add_signal_role_to_maps(
        &mut scope.signal_roles,
        &scope.declarations,
        ports,
        name,
        role,
    );
}

fn add_signal_role_to_maps(
    signal_roles: &mut BTreeMap<String, SignalAnalysis>,
    declarations: &BTreeMap<String, DeclAnalysis>,
    ports: &BTreeMap<String, PortAnalysis>,
    name: &str,
    role: SignalRole,
) {
    let declaration_span = declarations
        .get(name)
        .map(|decl| decl.span.clone())
        .or_else(|| ports.get(name).map(|port| port.span.clone()));
    signal_roles
        .entry(name.to_string())
        .or_insert_with(|| SignalAnalysis {
            declaration_span,
            roles: BTreeSet::new(),
        })
        .roles
        .insert(role);
}

fn mark_hierarchical_connection(
    expr: &Expr,
    scope: &mut ScopeAnalysis,
    ports: &BTreeMap<String, PortAnalysis>,
) {
    if let Some(name) = expr_local_symbol(expr) {
        add_signal_role(scope, name, SignalRole::HierarchicalConnection, ports);
    }
}

fn record_driver(
    scope: &mut ScopeAnalysis,
    target: &Expr,
    span: Span,
    source_order: usize,
    source: DriverSource,
) {
    scope.drivers.push(DriverAnalysis {
        span,
        source_order,
        target: render_expr(target),
        source,
    });
}

fn next_source_order(source_order: &mut usize) -> usize {
    let current = *source_order;
    *source_order += 1;
    current
}

fn render_expr(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Path(segments) => segments.join("::"),
        ExprKind::Integer(value) | ExprKind::Real(value) => value.clone(),
        ExprKind::Constant(kind) => match kind {
            ConstKind::Zero => "'0".to_string(),
            ConstKind::One => "'1".to_string(),
            ConstKind::Z => "'z".to_string(),
            ConstKind::X => "'x".to_string(),
        },
        ExprKind::Group(inner) => format!("({})", render_expr(inner)),
        ExprKind::Unary { op, expr } => format!("({} {})", unary_label(*op), render_expr(expr)),
        ExprKind::Binary { op, left, right } => format!(
            "({} {} {})",
            binary_label(*op),
            render_expr(left),
            render_expr(right)
        ),
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => format!(
            "(?: {} {} {})",
            render_expr(condition),
            render_expr(then_expr),
            render_expr(else_expr)
        ),
        ExprKind::Call { callee, args } => {
            let rendered_args = args
                .iter()
                .map(|arg| {
                    arg.as_ref()
                        .map(render_expr)
                        .unwrap_or_else(|| "_".to_string())
                })
                .collect::<Vec<_>>()
                .join(" ");
            if rendered_args.is_empty() {
                format!("(call {})", render_expr(callee))
            } else {
                format!("(call {} {})", render_expr(callee), rendered_args)
            }
        }
    }
}

fn render_delay(delay: &Delay) -> String {
    let parts = delay
        .values
        .iter()
        .map(|item| {
            item.as_ref()
                .map(render_expr)
                .unwrap_or_else(|| "_".to_string())
        })
        .collect::<Vec<_>>();
    format!("({})", parts.join(" "))
}

fn unary_label(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bitnot",
        UnaryOp::Plus => "plus",
        UnaryOp::Minus => "minus",
    }
}

fn binary_label(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::BitAnd => "and",
        BinaryOp::BitOr => "or",
        BinaryOp::BitXor => "xor",
        BinaryOp::BitNand => "nand",
        BinaryOp::BitNor => "nor",
        BinaryOp::BitXnor => "xnor",
        BinaryOp::LogicalAnd => "land",
        BinaryOp::LogicalOr => "lor",
        BinaryOp::Eq => "eq",
        BinaryOp::CaseEq => "caseeq",
        BinaryOp::Neq => "neq",
        BinaryOp::CaseNeq => "caseneq",
        BinaryOp::Less => "lt",
        BinaryOp::Greater => "gt",
    }
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

fn join_list(items: &[String]) -> String {
    if items.is_empty() {
        "<none>".to_string()
    } else {
        items.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::fs;

    fn analyze_path(path: &str) -> AnalysisReport {
        let path = Path::new(path);
        let input = fs::read_to_string(path).unwrap();
        analyze_file_structural(path, &input).unwrap()
    }

    fn parse_path(path: &str) -> Design {
        let path = Path::new(path);
        let input = fs::read_to_string(path).unwrap();
        parse_file(path, &input).unwrap()
    }

    fn analyze_catalog_source(source: &str) -> AnalyzeResult<AnalysisReport> {
        let design = parse_file(Path::new("catalog_test.sv"), source).unwrap();
        let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&design)).unwrap();
        analyze_design_with_catalog_structural(&design, &catalog)
    }

    fn assert_analysis_error(
        source: &str,
        expected_line: usize,
        expected_column: usize,
        expected_message: &str,
    ) {
        let diagnostic = analyze_catalog_source(source).unwrap_err();
        assert_eq!(diagnostic.span.path, Path::new("catalog_test.sv"));
        assert_eq!(diagnostic.span.line, expected_line);
        assert_eq!(diagnostic.span.column, expected_column);
        assert_eq!(diagnostic.message, expected_message);
    }

    #[test]
    fn analyzes_reference_cell() {
        let report = analyze_path("../sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv");
        assert_eq!(report.modules.len(), 1);
        let module = &report.modules[0];
        assert_eq!(
            module.inputs,
            vec!["clk", "clk_n", "ena", "ena_n", "s_n", "pch_n", "d"]
        );
        assert_eq!(module.outputs, vec!["q", "q_n", "d"]);
        assert_eq!(module.registers, vec!["ff1", "ff2", "q_n"]);
        assert_eq!(module.procedural_assignments.len(), 3);
        assert_eq!(module.continuous_assignments.len(), 1);
        assert_eq!(
            module
                .primitive_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bufif0"]
        );
        assert_eq!(module.symbols["d"].category, SymbolCategory::Port);
        assert_eq!(module.symbols["L_d"].category, SymbolCategory::Parameter);
        assert_eq!(module.symbols["ff1"].category, SymbolCategory::Declaration);
        assert_eq!(
            module.symbols["T_rise_d"].category,
            SymbolCategory::Localparam
        );
        assert_eq!(
            module.symbols["T_rise_buf1"].category,
            SymbolCategory::Specparam
        );
        assert_eq!(module.specify_paths.len(), 2);
        assert!(module.specify_paths.iter().all(|path| path.span.line > 0));
    }

    #[test]
    fn analyzes_simple_combinational_cell() {
        let report = analyze_path("../sv-cells/sm83/cells/and2.sv");
        let module = &report.modules[0];
        assert_eq!(module.inputs, vec!["in1", "in2"]);
        assert_eq!(module.outputs, vec!["y"]);
        assert!(module.registers.is_empty());
        assert_eq!(module.continuous_assignments.len(), 1);
        assert!(module.procedural_assignments.is_empty());
    }

    #[test]
    fn continuous_and_primitive_internal_nets_are_not_registers() {
        let report = analyze_path("../sv-cells/dmg_cpu_b/cells/muxi.sv");
        let module = &report.modules[0];
        assert!(module.registers.is_empty());
        assert_eq!(
            module.signal_roles["sel_n"].roles,
            BTreeSet::from([SignalRole::ContinuousDriven])
        );
        assert!(
            module.signal_roles["mux"]
                .roles
                .contains(&SignalRole::PrimitiveDriven)
        );
        assert!(
            !module.signal_roles["mux"]
                .roles
                .contains(&SignalRole::HierarchicalConnection)
        );
        assert_eq!(
            module
                .drivers
                .iter()
                .filter(|driver| driver.target == "mux")
                .count(),
            4
        );
    }

    #[test]
    fn generated_dff_branches_remain_distinct() {
        let report = analyze_path("../sv-cells/dmg_cpu_b/cells/dffr_cc.sv");
        let module = &report.modules[0];
        assert_eq!(module.generate_alternatives.len(), 1);
        assert!(module.registers.is_empty());
        assert!(module.drivers.is_empty());
        assert!(module.continuous_assignments.is_empty());
        assert!(module.procedural_assignments.is_empty());

        let alternative = &module.generate_alternatives[0];
        assert_eq!(alternative.condition.text, "nodelay");
        assert_eq!(alternative.then_branch.registers, vec!["ff", "q"]);
        assert_eq!(alternative.then_branch.continuous_assignments.len(), 1);
        assert_eq!(alternative.then_branch.procedural_assignments.len(), 5);
        assert_eq!(alternative.then_branch.drivers.len(), 8);

        let else_branch = alternative.else_branch.as_ref().unwrap();
        assert_eq!(else_branch.registers, vec!["mux1", "mux2"]);
        assert_eq!(else_branch.continuous_assignments.len(), 6);
        assert_eq!(else_branch.procedural_assignments.len(), 2);
        assert_eq!(else_branch.drivers.len(), 10);
        assert!(alternative.then_branch.declarations.contains_key("ff"));
        assert!(else_branch.declarations.contains_key("mux1"));
        assert!(!alternative.then_branch.declarations.contains_key("mux1"));
        assert!(!else_branch.declarations.contains_key("ff"));
    }

    #[test]
    fn timing_only_references_do_not_classify_inout_ports() {
        let source = r#"
module timing_only(input logic a, inout logic timing, output logic y);
  assign #(timing) y = a;
  specify
    (timing *> y) = (timing);
  endspecify
endmodule
"#;
        let report = analyze_file(Path::new("timing_only.sv"), source).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.inputs, vec!["a"]);
        assert_eq!(module.outputs, vec!["y"]);
        assert!(!module.ports["timing"].is_input);
        assert!(!module.ports["timing"].is_output);
        assert_eq!(module.specify_paths.len(), 1);
    }

    #[test]
    fn only_contracted_literal_initializers_classify_modeled_state() {
        let source = r#"
module initial_state(input logic d, output logic q, q_lit);
  initial q = d;
  initial q_lit = ('0);
endmodule
"#;
        let report = analyze_file(Path::new("initial_state.sv"), source).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.initial_assignments.len(), 2);
        assert_eq!(module.registers, vec!["q_lit"]);
        assert!(!module.signal_roles.contains_key("q"));
        assert_eq!(
            module.signal_roles["q_lit"].roles,
            BTreeSet::from([SignalRole::ModeledState])
        );
        assert_eq!(
            module
                .drivers
                .iter()
                .map(|driver| driver.target.as_str())
                .collect::<Vec<_>>(),
            vec!["q", "q_lit"]
        );
    }

    #[test]
    fn always_ff_without_sensitivity_is_stateful() {
        let source = r#"
module implicit_ff(input logic d, output logic q);
  always_ff q <= d;
endmodule
"#;
        let report = analyze_file(Path::new("implicit_ff.sv"), source).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.registers, vec!["q"]);
        assert_eq!(module.procedural_assignments.len(), 1);
        assert_eq!(
            module.procedural_assignments[0].kind,
            AssignmentKind::Procedural {
                state: true,
                source: ProceduralSource::AlwaysFf,
            }
        );
        assert_eq!(
            module.signal_roles["q"].roles,
            BTreeSet::from([SignalRole::ModeledState])
        );
    }

    #[test]
    fn resolves_full_adder_against_owned_catalog() {
        let full_add = parse_path("../sv-cells/dmg_cpu_b/cells/full_add.sv");
        let xor = parse_path("../sv-cells/dmg_cpu_b/cells/xor.sv");
        let nand2 = parse_path("../sv-cells/dmg_cpu_b/cells/nand2.sv");
        let catalog = ModuleCatalog::from_designs(&[full_add.clone(), xor, nand2]).unwrap();
        assert_eq!(
            catalog
                .modules
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["dmg_full_add", "dmg_nand2", "dmg_xor"]
        );
        let report = analyze_design_with_catalog_structural(&full_add, &catalog).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.instantiations.len(), 5);
        assert!(module.registers.is_empty());
        assert!(module.continuous_assignments.is_empty());
        assert!(module.procedural_assignments.is_empty());

        let mut input_connections = Vec::new();
        let mut output_connections = Vec::new();
        for instantiation in &module.instantiations {
            let InstantiationResolution::Resolved(resolved) = &instantiation.resolution else {
                panic!("ordinary full-adder instance was not resolved")
            };
            assert_eq!(resolved.parameter_bindings.len(), 1);
            let binding = &resolved.parameter_bindings[0];
            assert_eq!(binding.parameter, "L_y");
            assert_eq!(binding.source, ParameterBindingSource::Named);
            assert!(binding.span.line > 0);
            assert_eq!(resolved.connections.len(), 3);
            assert_eq!(
                resolved
                    .connections
                    .iter()
                    .map(|connection| connection.port.as_str())
                    .collect::<Vec<_>>(),
                vec!["y", "in1", "in2"]
            );
            for connection in &resolved.connections {
                assert_eq!(connection.source, ConnectionSource::Named);
                assert!(connection.span.line > 0);
                match connection.direction {
                    Direction::Input => input_connections
                        .push(connection.local_signal.as_deref().unwrap().to_string()),
                    Direction::Output => output_connections
                        .push(connection.local_signal.as_deref().unwrap().to_string()),
                    Direction::Inout => panic!("adder children have no inout ports"),
                }
            }
        }
        assert_eq!(output_connections, vec!["sum", "caxb", "cout", "ab", "axb"]);
        assert_eq!(
            input_connections,
            vec!["axb", "cin", "cin", "axb", "ab", "caxb", "b", "a", "a", "b"]
        );
        assert_eq!(
            module
                .drivers
                .iter()
                .map(|driver| driver.target.as_str())
                .collect::<Vec<_>>(),
            vec!["sum", "caxb", "cout", "ab", "axb"]
        );
        for name in ["sum", "cout", "axb", "ab", "caxb"] {
            assert!(
                module.signal_roles[name]
                    .roles
                    .contains(&SignalRole::HierarchicalConnection)
            );
            assert!(!module.registers.iter().any(|register| register == name));
        }
        assert_eq!(module.inputs, vec!["a", "b", "cin"]);
        assert_eq!(module.outputs, vec!["sum", "cout"]);
    }

    #[test]
    fn keeper_is_an_explicit_special_instance() {
        let design = parse_path("../sv-cells/dmg_cpu_b/cells/mux.sv");
        let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&design)).unwrap();
        let report = analyze_design_with_catalog_structural(&design, &catalog).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.instantiations.len(), 1);
        let InstantiationResolution::Special(special) = &module.instantiations[0].resolution else {
            panic!("keeper was not retained as a special instance")
        };
        assert_eq!(special.kind, SpecialInstanceKind::Keeper);
        assert_eq!(special.keeper.instance, "mux_keeper");
        assert_eq!(special.keeper.connection.target, "mux");
        assert_eq!(
            special.keeper.connection.expression.kind,
            ExprKind::Path(vec!["mux".into()])
        );
        assert!(
            module.signal_roles["mux"]
                .roles
                .contains(&SignalRole::KeeperDriven)
        );
        assert!(
            !module.signal_roles["mux"]
                .roles
                .contains(&SignalRole::HierarchicalConnection)
        );
        assert!(module.drivers.iter().any(|driver| {
            driver.target == "mux"
                && matches!(
                    &driver.source,
                    DriverSource::Keeper { instance } if instance == "mux_keeper"
                )
        }));
        assert!(module.registers.is_empty());
        assert!(report.requirements.iter().all(|requirement| {
            requirement.capability_id != "hierarchy.keeper"
                && requirement.milestone != TargetMilestone::M10Keeper
        }));
    }

    #[test]
    fn keeper_driver_remains_distinct_and_in_source_order() {
        let source = r#"module keeper_order(input logic a, en, inout logic held);
  assign held = a;
  bufif0 (held, a, en);
  keeper hold(held);
  assign held = en;
endmodule
"#;
        let report = analyze_catalog_source(source).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.registers, Vec::<String>::new());
        assert_eq!(module.inputs, vec!["a", "en"]);
        assert_eq!(module.outputs, vec!["held"]);
        assert_eq!(
            module.signal_roles["held"].roles,
            BTreeSet::from([
                SignalRole::ContinuousDriven,
                SignalRole::PrimitiveDriven,
                SignalRole::KeeperDriven,
            ])
        );
        assert_eq!(
            module
                .drivers
                .iter()
                .map(|driver| (driver.source_order, driver.target.as_str(), &driver.source))
                .collect::<Vec<_>>(),
            vec![
                (0, "held", &DriverSource::Continuous),
                (
                    1,
                    "held",
                    &DriverSource::Primitive {
                        name: "bufif0".into()
                    }
                ),
                (
                    2,
                    "held",
                    &DriverSource::Keeper {
                        instance: "hold".into()
                    }
                ),
                (3, "held", &DriverSource::Continuous),
            ]
        );
    }

    #[test]
    fn resolves_all_six_corpus_keeper_instances() {
        let cases = [
            ("../sv-cells/dmg_cpu_b/cells/mux.sv", "mux_keeper", "mux"),
            ("../sv-cells/dmg_cpu_b/cells/muxi.sv", "mux_keeper", "mux"),
            (
                "../sv-cells/dmg_cpu_b/cells/pad_xtal.sv",
                "clk_keeper",
                "clk",
            ),
            (
                "../sv-cells/sm83/cells/idu_bit0.sv",
                "aoi_y_keeper",
                "aoi_y",
            ),
            (
                "../sv-cells/sm83/cells/reg_wz_out.sv",
                "aoi_a_y_keeper",
                "aoi_a_y",
            ),
            (
                "../sv-cells/sm83/cells/dlatch_ee_irq.sv",
                "gated_q_keeper",
                "gated_q",
            ),
        ];
        for (path, expected_instance, expected_target) in cases {
            let design = parse_path(path);
            let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&design)).unwrap();
            let report = analyze_design_with_catalog(&design, &catalog).unwrap();
            let module = &report.modules[0];
            let special = module
                .instantiations
                .iter()
                .find_map(|instantiation| match &instantiation.resolution {
                    InstantiationResolution::Special(special) => Some(special),
                    InstantiationResolution::Unresolved | InstantiationResolution::Resolved(_) => {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("missing resolved keeper in {path}"));
            assert_eq!(special.keeper.instance, expected_instance, "{path}");
            assert_eq!(special.keeper.connection.target, expected_target, "{path}");
            assert!(
                module.signal_roles[expected_target]
                    .roles
                    .contains(&SignalRole::KeeperDriven),
                "{path}"
            );
            assert!(!module.registers.iter().any(|name| name == expected_target));
            assert!(module.drivers.iter().any(|driver| {
                driver.target == expected_target
                    && matches!(
                        &driver.source,
                        DriverSource::Keeper { instance } if instance == expected_instance
                    )
            }));
        }
    }

    #[test]
    fn rejects_malformed_keeper_forms_at_specific_spans() {
        let cases = [
            (
                "module bad(output logic y);\n  keeper #(1) hold(y);\nendmodule\n",
                2,
                12,
                "keeper instance `hold` does not accept parameter overrides",
            ),
            (
                "module bad(output logic y);\n  keeper hold(.target(y));\nendmodule\n",
                2,
                15,
                "keeper instance `hold` requires a positional connection",
            ),
            (
                "module bad(output logic y);\n  keeper hold();\nendmodule\n",
                2,
                3,
                "keeper instance `hold` requires exactly one positional connection",
            ),
            (
                "module bad(input logic a, output logic y);\n  keeper hold(y, a);\nendmodule\n",
                2,
                18,
                "keeper instance `hold` requires exactly one positional connection",
            ),
            (
                "module bad(input logic a, output logic y);\n  keeper hold(a & y);\nendmodule\n",
                2,
                15,
                "keeper instance `hold` target must be a scalar signal name",
            ),
            (
                "module bad(output logic y);\n  keeper hold(missing);\nendmodule\n",
                2,
                15,
                "unknown keeper target `missing` for instance `hold`",
            ),
        ];
        for (source, line, column, message) in cases {
            assert_analysis_error(source, line, column, message);
        }
    }

    #[test]
    fn resolves_hierarchy_inside_generate_scope_and_applies_inout_usage() {
        let source = r#"
module child #(parameter P = 0)(inout logic p, output logic o);
endmodule
module parent(input logic select, inout logic x);
  logic root_net;
  generate
    if (select) begin
      child u(.p(x), .o(root_net));
    end
  endgenerate
endmodule
"#;
        let report = analyze_catalog_source(source).unwrap();
        let parent = &report.modules[1];
        assert_eq!(parent.inputs, vec!["select", "x"]);
        assert_eq!(parent.outputs, vec!["x"]);
        assert!(parent.drivers.is_empty());
        assert!(parent.registers.is_empty());
        let branch = &parent.generate_alternatives[0].then_branch;
        assert_eq!(branch.instantiations.len(), 1);
        assert_eq!(branch.drivers.len(), 2);
        assert_eq!(branch.drivers[0].target, "x");
        assert_eq!(branch.drivers[1].target, "root_net");
        assert!(branch.registers.is_empty());
        let InstantiationResolution::Resolved(resolved) = &branch.instantiations[0].resolution
        else {
            panic!("branch-local child was not resolved")
        };
        assert_eq!(resolved.parameter_bindings.len(), 1);
        assert_eq!(
            resolved.parameter_bindings[0].source,
            ParameterBindingSource::Default
        );
        assert_eq!(resolved.connections[0].direction, Direction::Inout);
        assert_eq!(resolved.connections[0].local_signal.as_deref(), Some("x"));
        assert_eq!(resolved.connections[1].direction, Direction::Output);
        assert_eq!(
            resolved.connections[1].local_signal.as_deref(),
            Some("root_net")
        );
        assert_eq!(
            branch.signal_roles["root_net"]
                .declaration_span
                .as_ref()
                .map(|span| span.line),
            Some(5)
        );
    }

    #[test]
    fn preserves_omitted_positional_parameter_bindings() {
        let source = r#"
module child #(parameter P = 1, parameter Q = 2)(input logic i, output logic o);
endmodule
module parent(input logic i, output logic o);
  child #(, 3) u(i, o);
endmodule
"#;
        let report = analyze_catalog_source(source).unwrap();
        let InstantiationResolution::Resolved(resolved) =
            &report.modules[1].instantiations[0].resolution
        else {
            panic!("positional child was not resolved")
        };
        assert_eq!(resolved.parameter_bindings.len(), 2);
        assert_eq!(resolved.parameter_bindings[0].parameter, "P");
        assert_eq!(
            resolved.parameter_bindings[0].source,
            ParameterBindingSource::OmittedPositional { index: 0 }
        );
        assert_eq!(
            resolved.parameter_bindings[0].expression.kind,
            ExprKind::Integer("1".into())
        );
        assert_eq!(resolved.parameter_bindings[1].parameter, "Q");
        assert_eq!(
            resolved.parameter_bindings[1].source,
            ParameterBindingSource::Positional { index: 1 }
        );
        assert_eq!(
            resolved
                .connections
                .iter()
                .map(|connection| &connection.source)
                .collect::<Vec<_>>(),
            vec![
                &ConnectionSource::Positional { index: 0 },
                &ConnectionSource::Positional { index: 1 }
            ]
        );
    }

    #[test]
    fn hierarchical_drivers_remain_in_source_order_with_local_drivers() {
        let source = r#"
module child(output logic o);
endmodule
module parent(output logic a, b, c);
  assign a = '0;
  child u(.o(b));
  assign c = '1;
endmodule
"#;
        let report = analyze_catalog_source(source).unwrap();
        let parent = &report.modules[1];
        assert_eq!(
            parent
                .drivers
                .iter()
                .map(|driver| (driver.source_order, driver.target.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "a"), (1, "b"), (2, "c")]
        );
    }

    #[test]
    fn support_classification_covers_milestones_four_through_eleven() {
        let supported = analyze_file(Path::new("empty.sv"), "module empty; endmodule\n").unwrap();
        assert_eq!(supported.disposition, AnalysisDisposition::Supported);
        assert!(supported.requirements.is_empty());

        let full_add = parse_path("../sv-cells/dmg_cpu_b/cells/full_add.sv");
        let xor = parse_path("../sv-cells/dmg_cpu_b/cells/xor.sv");
        let nand2 = parse_path("../sv-cells/dmg_cpu_b/cells/nand2.sv");
        let catalog = ModuleCatalog::from_designs(&[full_add.clone(), xor, nand2]).unwrap();
        let hierarchy = analyze_design_with_catalog_structural(&full_add, &catalog).unwrap();
        assert_eq!(hierarchy.disposition, AnalysisDisposition::Supported);
        assert!(hierarchy.requirements.is_empty());

        let mux = parse_path("../sv-cells/dmg_cpu_b/cells/mux.sv");
        let keeper_structural = analyze_design_structural(&mux);

        let reports = [
            (
                analyze_path("../sv-cells/dmg_cpu_b/cells/and2.sv"),
                TargetMilestone::M4FlatCombinational,
            ),
            (
                analyze_path("../sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv"),
                TargetMilestone::M5StatefulProcedural,
            ),
            (
                analyze_path("../sv-cells/dmg_cpu_b/cells/muxi.sv"),
                TargetMilestone::M6DriversAndStrength,
            ),
            (
                analyze_path("../sv-cells/dmg_cpu_b/cells/and2.sv"),
                TargetMilestone::M7SymbolicTiming,
            ),
            (
                analyze_path("../sv-cells/dmg_cpu_b/cells/dffr_cc.sv"),
                TargetMilestone::M8GenerateSelection,
            ),
            (keeper_structural, TargetMilestone::M10Keeper),
            (
                analyze_path("../sv-cells/sm83/cells/irq_prio_bit0.sv"),
                TargetMilestone::M11Transistors,
            ),
        ];
        for (report, milestone) in reports {
            assert_eq!(report.disposition, AnalysisDisposition::Deferred);
            assert!(
                report
                    .requirements
                    .iter()
                    .any(|requirement| requirement.milestone == milestone),
                "missing requirement for {}",
                milestone.label()
            );
        }
    }

    #[test]
    fn invalid_initial_forms_fail_at_their_specific_expression_spans() {
        let nonliteral = analyze_file(
            Path::new("nonliteral_initial.sv"),
            "module bad(input logic d, output logic q);\n  initial q = d;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(nonliteral.disposition, AnalysisDisposition::Failed);
        assert!(nonliteral.modules[0].registers.is_empty());
        assert_eq!(nonliteral.diagnostics.len(), 1);
        assert_eq!(nonliteral.diagnostics[0].span.line, 2);
        assert_eq!(nonliteral.diagnostics[0].span.column, 15);
        assert_eq!(
            nonliteral.diagnostics[0].message,
            "initial assignment value must be a contracted literal (0, 1, '0, '1, 'x, or 'z)"
        );

        let integer_two = analyze_file(
            Path::new("integer_two_initial.sv"),
            "module bad(output logic q);\n  initial q = 2;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(integer_two.disposition, AnalysisDisposition::Failed);
        assert!(integer_two.modules[0].registers.is_empty());
        assert_eq!(integer_two.diagnostics[0].span.line, 2);
        assert_eq!(integer_two.diagnostics[0].span.column, 15);

        let nonscalar = analyze_file(
            Path::new("nonscalar_initial.sv"),
            "module bad(input logic d, output logic q);\n  initial q & d = '0;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(nonscalar.disposition, AnalysisDisposition::Failed);
        assert!(nonscalar.modules[0].registers.is_empty());
        assert_eq!(nonscalar.diagnostics.len(), 1);
        assert_eq!(nonscalar.diagnostics[0].span.line, 2);
        assert_eq!(nonscalar.diagnostics[0].span.column, 11);
        assert_eq!(
            nonscalar.diagnostics[0].message,
            "initial assignment target must be a scalar local signal"
        );

        let literal = analyze_file(
            Path::new("literal_initial.sv"),
            "module okay(output logic q0, q1, qx);\n  initial q0 = 0;\n  initial q1 = 1;\n  initial qx = 'x;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(literal.disposition, AnalysisDisposition::Deferred);
        assert_eq!(literal.modules[0].registers, vec!["q0", "q1", "qx"]);
        assert!(literal.diagnostics.is_empty());
        assert!(
            literal.requirements.iter().any(|requirement| {
                requirement.milestone == TargetMilestone::M5StatefulProcedural
            })
        );
    }

    #[test]
    fn duplicate_symbols_preserve_first_entry_and_fail_at_duplicate_span() {
        let cross_category = analyze_file(
            Path::new("cross_category.sv"),
            "module conflict(input logic x);\n  logic x;\nendmodule\n",
        )
        .unwrap();
        assert_eq!(cross_category.disposition, AnalysisDisposition::Failed);
        let module = &cross_category.modules[0];
        assert_eq!(module.symbols["x"].category, SymbolCategory::Port);
        assert!(module.ports.contains_key("x"));
        assert!(!module.declarations.contains_key("x"));
        assert_eq!(cross_category.diagnostics.len(), 1);
        assert_eq!(cross_category.diagnostics[0].span.line, 2);
        assert_eq!(cross_category.diagnostics[0].span.column, 3);
        assert_eq!(
            cross_category.diagnostics[0].message,
            "duplicate symbol `x` as declaration; first declared as port at cross_category.sv:1:17"
        );

        let same_category = analyze_file(
            Path::new("same_category.sv"),
            "module duplicate(input logic x, x); endmodule\n",
        )
        .unwrap();
        assert_eq!(same_category.disposition, AnalysisDisposition::Failed);
        let module = &same_category.modules[0];
        assert_eq!(module.symbols.len(), 1);
        assert_eq!(module.ports.len(), 1);
        assert_eq!(same_category.diagnostics.len(), 1);
        assert_eq!(same_category.diagnostics[0].span.line, 1);
        assert_eq!(same_category.diagnostics[0].span.column, 18);
        assert_eq!(
            same_category.diagnostics[0].message,
            "duplicate symbol `x` as port; first declared as port at same_category.sv:1:18"
        );

        let all_categories = analyze_file(
            Path::new("all_categories.sv"),
            "module categories #(parameter X = 0)(input logic y);\n  localparam X = 1;\n  specify\n    specparam y = 1;\n  endspecify\nendmodule\n",
        )
        .unwrap();
        assert_eq!(all_categories.disposition, AnalysisDisposition::Failed);
        let module = &all_categories.modules[0];
        assert_eq!(module.symbols["X"].category, SymbolCategory::Parameter);
        assert_eq!(module.symbols["y"].category, SymbolCategory::Port);
        assert!(!module.localparams.contains_key("X"));
        assert!(!module.specparams.contains_key("y"));
        assert_eq!(all_categories.diagnostics.len(), 2);
        assert_eq!(all_categories.diagnostics[0].span.line, 2);
        assert_eq!(all_categories.diagnostics[0].span.column, 3);
        assert_eq!(
            all_categories.diagnostics[0].message,
            "duplicate symbol `X` as localparam; first declared as parameter at all_categories.sv:1:21"
        );
        assert_eq!(all_categories.diagnostics[1].span.line, 4);
        assert_eq!(all_categories.diagnostics[1].span.column, 5);
        assert_eq!(
            all_categories.diagnostics[1].message,
            "duplicate symbol `y` as specparam; first declared as port at all_categories.sv:1:38"
        );
    }

    #[test]
    fn hierarchy_reports_unknown_and_duplicate_named_parameters() {
        assert_analysis_error(
            "module child #(parameter P = 0)(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child #(.BAD(1)) u(.i(i), .o(o));\nendmodule\n",
            3,
            11,
            "unknown parameter `BAD` on module `child`",
        );
        assert_analysis_error(
            "module child #(parameter P = 0)(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child #(.P(1), .P(2)) u(.i(i), .o(o));\nendmodule\n",
            3,
            18,
            "duplicate parameter override `P`",
        );
    }

    #[test]
    fn hierarchy_reports_unknown_duplicate_and_mixed_named_ports() {
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(.bad(i), .o(o));\nendmodule\n",
            3,
            11,
            "unknown port `bad` on module `child`",
        );
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(.i(i), .i(i), .o(o));\nendmodule\n",
            3,
            18,
            "duplicate port connection `i`",
        );
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(.i(i), o);\nendmodule\n",
            3,
            18,
            "cannot mix named and positional port connections",
        );
    }

    #[test]
    fn hierarchy_reports_positional_parameter_and_port_shape_errors() {
        assert_analysis_error(
            "module child #(parameter P = 0)(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child #(.P(1), 2) u(i, o);\nendmodule\n",
            3,
            18,
            "cannot mix named and positional parameter overrides",
        );
        assert_analysis_error(
            "module child #(parameter P = 0)(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child #(1, 2) u(i, o);\nendmodule\n",
            3,
            14,
            "too many positional parameter overrides for module `child`: expected at most 1",
        );
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(i);\nendmodule\n",
            3,
            3,
            "wrong positional port arity for module `child`: expected 2, found 1",
        );
    }

    #[test]
    fn hierarchy_rejects_nonassignable_outputs_and_unknown_modules() {
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(.i(i), .o(!o));\nendmodule\n",
            3,
            18,
            "connection to output port `o` must be an assignable scalar local signal",
        );
        assert_analysis_error(
            "module child(input logic i, output logic o); endmodule\n\
             module parent(input logic i, output logic o);\n  child u(.i(i), .o(undeclared));\nendmodule\n",
            3,
            18,
            "connection to output port `o` references undeclared local signal `undeclared`",
        );
        assert_analysis_error(
            "module parent(input logic i, output logic o);\n  missing u(i, o);\nendmodule\n",
            2,
            3,
            "unknown instantiated module `missing` for instance `u`",
        );
    }

    #[test]
    fn hierarchy_rejects_direct_and_indirect_recursion() {
        assert_analysis_error(
            "module recursive(input logic i);\n  recursive self(i);\nendmodule\n",
            2,
            3,
            "recursive module reference from `recursive` through `recursive`",
        );
        assert_analysis_error(
            "module a(input logic i);\n  b b_inst(i);\nendmodule\n\
             module b(input logic i);\n  c c_inst(i);\nendmodule\n\
             module c(input logic i);\n  a a_inst(i);\nendmodule\n",
            2,
            3,
            "recursive module reference from `a` through `b`",
        );
    }
}
