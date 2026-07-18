use crate::analyze::{
    ModuleCatalog, analyze_design_structural, analyze_design_with_catalog_and_generate_mode,
    resolve_keeper_ast_instantiation, sensitivity_is_stateful,
};
use crate::ast::*;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Span};
use crate::elaborate::{GenerateMode, elaborate_design};
use crate::ir::{
    Assignment, Cell, CellItem, Expr, LoweredModule, StrengthPair, TimingOperator, ValueOperator,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub type LowerResult<T> = Result<T, Diagnostic>;
type SvExpr = crate::ast::Expr;

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

/// Lowers a design that has already had its generate configuration selected.
/// The supplied analysis must describe that exact elaborated design.
pub fn lower_elaborated_design(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
) -> LowerResult<LoweredModule> {
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
    lower_module(module, module_analysis)
}

fn lower_module(
    module: &Module,
    analysis: &crate::analyze::ModuleAnalysis,
) -> LowerResult<LoweredModule> {
    let mut lowerer = Lowerer::new(module, analysis);
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
}

impl ProceduralContext {
    fn combinational() -> Self {
        Self {
            mode: ProceduralMode::Combinational,
            condition: None,
        }
    }

    fn stateful(condition: Option<Expr>) -> Self {
        Self {
            mode: ProceduralMode::Stateful,
            condition,
        }
    }
}

struct Lowerer<'a> {
    module: &'a Module,
    cell: Cell,
    timing_alias_sources: BTreeMap<String, TimingAliasSource>,
    timing_aliases: BTreeMap<String, Expr>,
    timing_alias_stack: Vec<String>,
    specify_delays: BTreeMap<String, Vec<SpecifyDelay>>,
    ignored_additional_specify_targets: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    reserved_names: BTreeSet<String>,
    signal_names: BTreeSet<String>,
    signal_spans: BTreeMap<String, Span>,
    next_temp_index: usize,
}

#[derive(Debug, Clone)]
struct TimingAliasSource {
    span: Span,
    value: SvExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpecifyDelay {
    path_span: Span,
    delay: Expr,
}

impl<'a> Lowerer<'a> {
    fn new(module: &'a Module, analysis: &crate::analyze::ModuleAnalysis) -> Self {
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
        Self {
            module,
            cell: Cell {
                name: module.name.clone(),
                inputs: analysis.inputs.clone(),
                outputs: analysis.outputs.clone(),
                registers: analysis.registers.clone(),
                items: Vec::new(),
            },
            timing_alias_sources: BTreeMap::new(),
            timing_aliases: BTreeMap::new(),
            timing_alias_stack: Vec::new(),
            specify_delays: BTreeMap::new(),
            ignored_additional_specify_targets: BTreeSet::new(),
            diagnostics: Vec::new(),
            reserved_names,
            signal_names,
            signal_spans,
            next_temp_index: 0,
        }
    }

    fn lower_module(&mut self) -> LowerResult<LoweredModule> {
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

        Ok(LoweredModule {
            cell: self.cell.clone(),
            timing_aliases: self.timing_aliases.clone(),
            diagnostics: self.diagnostics.clone(),
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

    fn resolve_timing_alias(&mut self, name: &str, reference_span: &Span) -> LowerResult<Expr> {
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
                for control in &path.controls {
                    if scalar_expr_symbol(control).is_none() {
                        return Err(Diagnostic::new(
                            control.span.clone(),
                            "specify path control must be a scalar symbol",
                        ));
                    }
                }
                let target = scalar_expr_symbol(&path.target).ok_or_else(|| {
                    Diagnostic::new(
                        path.target.span.clone(),
                        "specify path target must be a scalar symbol",
                    )
                })?;
                let delay = self.lower_timing_tuple(&path.span, &path.delays)?;
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

    fn source_delay_for(&mut self, target: &str, explicit: Option<&Delay>) -> LowerResult<Expr> {
        match explicit {
            Some(delay) => self.lower_timing_expr_from_delay(delay),
            None => Ok(self.specify_delay_for(target)),
        }
    }

    fn specify_delay_for(&mut self, target: &str) -> Expr {
        let Some(matches) = self.specify_delays.get(target) else {
            return Expr::atom("0");
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
                    "additional control-dependent specify path for target `{target}` is intentionally ignored because the one-delay cell DSL selects the first source-ordered path for the target"
                ),
            ));
        }
        first
    }

    fn lower_item(&mut self, item: &Item) -> LowerResult<()> {
        match &item.kind {
            ItemKind::Assign(assign) => {
                self.lower_continuous_assign(assign)?;
                Ok(())
            }
            ItemKind::Primitive(call) => self.lower_primitive_call(call),
            ItemKind::Initial(stmt) => self.lower_initial(item, stmt),
            ItemKind::AlwaysLatch(always) => {
                let condition = always
                    .condition
                    .as_ref()
                    .map(|expr| self.lower_expr(expr))
                    .transpose()?;
                self.lower_procedural_body(&always.body, ProceduralContext::stateful(condition))
            }
            ItemKind::Always(always) => {
                let stateful = matches!(always.kind, AlwaysKind::Ff)
                    || always
                        .sensitivity
                        .as_ref()
                        .map(|sensitivity| sensitivity_is_stateful(sensitivity, always.kind))
                        .unwrap_or(false);
                let context = if stateful {
                    ProceduralContext::stateful(None)
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
            Expr::atom("0"),
            &keeper.connection.span,
        )
    }

    fn lower_initial(&mut self, item: &Item, stmt: &AssignStmt) -> LowerResult<()> {
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
        if !is_contracted_initial_literal(&stmt.value) {
            return Err(Diagnostic::new(
                stmt.value.span.clone(),
                "initial assignment value must be a contracted literal (0, 1, '0, '1, 'x, or 'z)",
            ));
        }
        self.diagnostics.push(Diagnostic::intentional_ignore(
            item.span.clone(),
            "literal initial value/event is intentionally omitted because the cell model has no initial event queue",
        ));
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
                    ProceduralContext::stateful(Some(next_condition)),
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
        let delay = self.source_delay_for(&target, None)?;
        self.emit_assignment(target, expr, delay, &stmt.span)
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
        let delay = self.source_delay_for(&target, assign.delay.as_ref())?;
        self.emit_assignment(target, expr, delay, &assign.span)
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
        let delay = self.source_delay_for(&drain, call.delay.as_ref())?;
        self.emit_assignment(drain, expr, delay, &call.span)
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
        let delay = self.source_delay_for(&target, call.delay.as_ref())?;
        self.emit_assignment(target, expr, delay, &call.span)
    }

    fn emit_assignment(
        &mut self,
        target: String,
        expr: Expr,
        delay: Expr,
        source_span: &Span,
    ) -> LowerResult<()> {
        let expr = self.flatten_value_root(expr, source_span)?;
        self.push_validated_assignment(target, expr, delay, source_span)
    }

    fn flatten_value_root(&mut self, expr: Expr, source_span: &Span) -> LowerResult<Expr> {
        let Expr::List(items) = expr else {
            return Ok(expr);
        };
        let mut items = items.into_iter();
        let head = items.next().ok_or_else(|| {
            Diagnostic::new(source_span.clone(), "value operator list must not be empty")
        })?;
        let Expr::Atom(head) = head else {
            return Err(Diagnostic::new(
                source_span.clone(),
                "value operator must be an atom",
            ));
        };
        let operator = ValueOperator::parse(&head).ok_or_else(|| {
            Diagnostic::new(
                source_span.clone(),
                format!("uncontracted value operator `{head}`"),
            )
        })?;
        let operands = items.collect::<Vec<_>>();
        if !operator.accepts_arity(operands.len()) {
            return Err(Diagnostic::new(
                source_span.clone(),
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
                    let nested = self.flatten_value_root(operand, source_span)?;
                    let temporary = self.allocate_temporary();
                    self.push_validated_assignment(
                        temporary.clone(),
                        nested,
                        Expr::atom("0"),
                        source_span,
                    )?;
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

    fn push_validated_assignment(
        &mut self,
        target: String,
        expr: Expr,
        delay: Expr,
        source_span: &Span,
    ) -> LowerResult<()> {
        let assignment = Assignment {
            target,
            expr,
            delay,
        };
        assignment.validate().map_err(|error| {
            Diagnostic::new(
                source_span.clone(),
                format!("invalid lowered assignment: {error}"),
            )
        })?;
        self.cell.items.push(CellItem::Assignment(assignment));
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

    fn lower_timing_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Path(segments) => {
                if segments.len() == 1 {
                    let name = &segments[0];
                    if self.timing_alias_sources.contains_key(name) {
                        return self.resolve_timing_alias(name, &expr.span);
                    }
                }
                Ok(Expr::atom(segments.join("::")))
            }
            ExprKind::Integer(value) | ExprKind::Real(value) => Ok(Expr::atom(value.clone())),
            ExprKind::Constant(kind) => Ok(Expr::atom(match kind {
                ConstKind::Zero => "0",
                ConstKind::One => "1",
                ConstKind::Z => "z",
                ConstKind::X => "x",
            })),
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
                Ok(Expr::timing(
                    operator,
                    vec![Expr::atom("0"), self.lower_timing_expr(operand)?],
                ))
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
                Ok(Expr::timing(
                    operator,
                    vec![
                        self.lower_timing_expr(left)?,
                        self.lower_timing_expr(right)?,
                    ],
                ))
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => Ok(Expr::timing(
                TimingOperator::Mux,
                vec![
                    self.lower_timing_expr(condition)?,
                    self.lower_timing_expr(then_expr)?,
                    self.lower_timing_expr(else_expr)?,
                ],
            )),
            ExprKind::Call { callee, args } => self.lower_timing_call(callee, args),
        }
    }

    fn lower_timing_expr_from_delay(&mut self, delay: &Delay) -> LowerResult<Expr> {
        self.lower_timing_tuple(&delay.span, &delay.values)
    }

    fn lower_timing_tuple(
        &mut self,
        tuple_span: &Span,
        values: &[Option<SvExpr>],
    ) -> LowerResult<Expr> {
        for (index, ignored) in values.iter().enumerate().skip(1) {
            let span = ignored
                .as_ref()
                .map(|expression| expression.span.clone())
                .unwrap_or_else(|| tuple_span.clone());
            self.diagnostics.push(Diagnostic::intentional_ignore(
                span,
                format!(
                    "delay tuple entry {} is intentionally ignored because the cell model selects only entry 1",
                    index + 1
                ),
            ));
        }
        let Some(first) = values.first() else {
            return Err(Diagnostic::new(
                tuple_span.clone(),
                "delay tuple must contain a first entry",
            ));
        };
        let first = first.as_ref().ok_or_else(|| {
            Diagnostic::new(
                tuple_span.clone(),
                "explicitly omitted first delay tuple entry is unsupported",
            )
        })?;
        self.lower_timing_expr(first)
    }

    fn lower_timing_call(&mut self, callee: &SvExpr, args: &[Option<SvExpr>]) -> LowerResult<Expr> {
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
                Ok(Expr::timing(
                    TimingOperator::Elmore,
                    vec![
                        Expr::timing(TimingOperator::Wire, vec![self.lower_timing_expr(wire)?]),
                        self.lower_timing_resistance(resistance)?,
                    ],
                ))
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

    fn lower_timing_resistance(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
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
    ) -> LowerResult<Expr> {
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
        Ok(Expr::timing(operator, vec![value]))
    }

    fn extract_unit_factor(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
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

    fn lower_resistance_factor(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Group(inner) => self.lower_resistance_factor(inner),
            ExprKind::Path(segments) if segments.len() == 1 && segments[0] == "L_unit" => {
                Ok(Expr::atom("1"))
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

fn is_contracted_initial_literal(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Constant(ConstKind::Zero | ConstKind::One | ConstKind::X | ConstKind::Z) => true,
        ExprKind::Integer(value) => matches!(value.as_str(), "0" | "1"),
        ExprKind::Group(inner) => is_contracted_initial_literal(inner),
        _ => false,
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
    use crate::serialize::render_expr;
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
                    render_expr(&assignment.delay),
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

    #[test]
    fn literal_initial_is_a_visible_non_failing_omission_and_emits_no_assignment() {
        let lowered =
            lower_snippet("module sample(output logic q);\n  initial q = ('0);\nendmodule\n")
                .unwrap();
        assert_eq!(lowered.cell.registers, vec!["q"]);
        assert!(assignment_strings(&lowered).is_empty());
        assert_eq!(lowered.diagnostics.len(), 1);
        assert_eq!(
            lowered.diagnostics[0].kind,
            DiagnosticKind::IntentionalIgnore
        );
        assert_eq!(lowered.diagnostics[0].span, Span::new("snippet.sv", 2, 3));
        assert_eq!(
            lowered.diagnostics[0].message,
            "literal initial value/event is intentionally omitted because the cell model has no initial event queue"
        );
        lowered.cell.validate().unwrap();
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
        assert_eq!(blocking.cell.registers, vec!["q"]);
        assert_eq!(nonblocking.cell.registers, vec!["q"]);
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
        assert_eq!(lowered.cell.registers, vec!["q"]);
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
        assert_eq!(lowered.cell.registers, vec!["ff1", "ff2", "q_n"]);
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
        assert_eq!(lowered.cell.registers, vec!["q", "q_n"]);
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
        assert_eq!(lowered.cell.registers, vec!["q"]);
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
    fn delay_tuples_select_exactly_the_first_entry() {
        for (delay, expected, ignored) in [
            ("#(1)", "1", 0),
            ("#(1, 2)", "1", 1),
            ("#(1, 2, 3)", "1", 2),
        ] {
            let input = format!(
                "module sample(input logic a, output logic y); assign {delay} y = a; endmodule"
            );
            let lowered = lower_snippet(&input).unwrap();
            assert_eq!(assignment_strings(&lowered)[0].2, expected);
            assert_eq!(lowered.diagnostics.len(), ignored);
            assert!(
                lowered
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
            );
        }
        let lowered =
            lower_snippet("module sample(input logic a, output logic y); assign y = a; endmodule")
                .unwrap();
        assert_eq!(assignment_strings(&lowered)[0].2, "0");
        assert!(lowered.diagnostics.is_empty());
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
            assignment_strings(&lowered),
            vec![
                ("y".into(), "a".into(), "T_first".into()),
                ("y".into(), "b".into(), "T_first".into()),
            ]
        );
        let [diagnostic] = lowered.diagnostics.as_slice() else {
            panic!("expected exactly one additional-path diagnostic")
        };
        assert_eq!(diagnostic.kind, DiagnosticKind::IntentionalIgnore);
        assert_eq!(diagnostic.span, Span::new("snippet.sv", 6, 5));
        assert_eq!(
            diagnostic.message,
            "additional control-dependent specify path for target `y` is intentionally ignored because the one-delay cell DSL selects the first source-ordered path for the target"
        );
        assert!(!DiagnosticPolicy::new(false).is_failure(diagnostic));
        assert!(!DiagnosticPolicy::new(true).is_failure(diagnostic));
    }

    #[test]
    fn symbolic_precharge_and_high_z_tuples_keep_only_entry_zero() {
        let lowered = lower_snippet(
            "module sample(input logic a, ena_n, output logic y0, y1);\n\
             bufif0 #(T_rise, T_Z, T_Z) (y0, a, ena_n);\n\
             assign #(T_Z, T_fall, T_off) y1 = a;\n\
             endmodule",
        )
        .unwrap();
        assert_eq!(
            assignment_strings(&lowered),
            vec![
                ("y0".into(), "(bufif0 a ena_n)".into(), "T_rise".into()),
                ("y1".into(), "a".into(), "T_Z".into()),
            ]
        );
        assert_eq!(lowered.diagnostics.len(), 4);
    }

    #[test]
    fn omitted_later_delay_entries_are_visible_non_failing_ignores() {
        let lowered = lower_snippet(
            "module sample(input logic a, output logic y); assign #(1, , 3) y = a; endmodule",
        )
        .unwrap();
        assert_eq!(assignment_strings(&lowered)[0].2, "1");
        assert_eq!(lowered.diagnostics.len(), 2);
        assert_eq!(lowered.diagnostics[0].span, Span::new("snippet.sv", 1, 55));
        assert_eq!(lowered.diagnostics[1].span, Span::new("snippet.sv", 1, 61));
        assert!(
            lowered
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
        );
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
    fn explicitly_omitted_first_delay_entry_is_an_error() {
        let error = lower_snippet(
            "module sample(input logic a, output logic y); assign #(, 2) y = a; endmodule",
        )
        .unwrap_err();
        assert!(error.message.contains("omitted first delay"));
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
        assert_eq!(lowered.cell.registers, Vec::<String>::new());
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
    fn transistor_delays_use_first_explicit_entry_or_specify_fallback() {
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
            assignment_strings(&lowered),
            vec![
                ("explicit".into(), "(nmos a g)".into(), "D_first".into()),
                ("fallback".into(), "(pmos a g)".into(), "S_fallback".into()),
            ]
        );
        assert_eq!(lowered.diagnostics.len(), 2);
        assert!(lowered.diagnostics.iter().all(|diagnostic| {
            diagnostic.kind == DiagnosticKind::IntentionalIgnore
                && diagnostic.message.contains(
                    "is intentionally ignored because the cell model selects only entry 1",
                )
        }));
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
}
