use crate::analyze::{analyze_design, sensitivity_is_stateful};
use crate::ast::*;
use crate::diagnostic::{Diagnostic, Span};
use crate::ir::{Assignment, Cell, CellItem, Expr, LoweredModule, TimingOperator, ValueOperator};
use std::collections::BTreeMap;
use std::path::Path;

pub type LowerResult<T> = Result<T, Diagnostic>;
type SvExpr = crate::ast::Expr;

pub fn lower_file(path: &Path, input: &str) -> LowerResult<LoweredModule> {
    let design = crate::parser::parse_file(path, input)?;
    let analysis = analyze_design(&design);
    lower_design(&design, &analysis)
}

pub fn lower_design(
    design: &Design,
    analysis: &crate::analyze::AnalysisReport,
) -> LowerResult<LoweredModule> {
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

struct Lowerer<'a> {
    module: &'a Module,
    cell: Cell,
    timing_aliases: BTreeMap<String, Expr>,
}

impl<'a> Lowerer<'a> {
    fn new(module: &'a Module, analysis: &crate::analyze::ModuleAnalysis) -> Self {
        Self {
            module,
            cell: Cell {
                name: module.name.clone(),
                inputs: analysis.inputs.clone(),
                outputs: analysis.outputs.clone(),
                registers: analysis.registers.clone(),
                items: Vec::new(),
            },
            timing_aliases: BTreeMap::new(),
        }
    }

    fn lower_module(&mut self) -> LowerResult<LoweredModule> {
        self.collect_timing_aliases()?;
        for item in &self.module.items {
            self.lower_item(item)?;
        }

        Ok(LoweredModule {
            cell: self.cell.clone(),
            timing_aliases: self.timing_aliases.clone(),
        })
    }

    fn collect_timing_aliases(&mut self) -> LowerResult<()> {
        for item in &self.module.items {
            match &item.kind {
                ItemKind::Decl(decl)
                    if matches!(decl.kind, DeclKind::Localparam | DeclKind::Specparam) =>
                {
                    if let Some(value) = &decl.value {
                        for name in &decl.names {
                            let lowered = self.lower_timing_expr(value)?;
                            self.timing_aliases.insert(name.clone(), lowered);
                        }
                    }
                }
                ItemKind::Specify(specify) => {
                    for specify_item in &specify.items {
                        if let SpecifyItem::Specparam(param) = specify_item {
                            let lowered = self.lower_timing_expr(&param.value)?;
                            self.timing_aliases.insert(param.name.clone(), lowered);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn lower_item(&mut self, item: &Item) -> LowerResult<()> {
        match &item.kind {
            ItemKind::Assign(assign) => {
                self.lower_continuous_assign(assign)?;
                Ok(())
            }
            ItemKind::Primitive(call) => self.lower_primitive_call(call),
            ItemKind::Initial(_) => Ok(()),
            ItemKind::AlwaysLatch(always) => {
                let condition = always
                    .condition
                    .as_ref()
                    .map(|expr| self.lower_expr(expr))
                    .transpose()?;
                self.lower_procedural_body(&always.body, condition, true)
            }
            ItemKind::Always(always) => {
                let stateful = matches!(always.kind, AlwaysKind::Ff)
                    || always
                        .sensitivity
                        .as_ref()
                        .map(|sensitivity| sensitivity_is_stateful(sensitivity, always.kind))
                        .unwrap_or(false);
                self.lower_procedural_body(&always.body, None, stateful)
            }
            ItemKind::Specify(_) | ItemKind::Decl(_) | ItemKind::Import(_) | ItemKind::Empty => {
                Ok(())
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

    fn lower_procedural_body(
        &mut self,
        item: &Item,
        condition: Option<Expr>,
        hold_on_false: bool,
    ) -> LowerResult<()> {
        match &item.kind {
            ItemKind::ProcAssign(stmt) => {
                self.lower_procedural_assign(stmt, condition.as_ref(), hold_on_false)
            }
            ItemKind::Block(block) | ItemKind::Generate(block) => {
                for child in &block.items {
                    self.lower_procedural_body(child, condition.clone(), hold_on_false)?;
                }
                Ok(())
            }
            ItemKind::If(stmt) => {
                if let Some(else_branch) = &stmt.else_branch {
                    return Err(Diagnostic::new(
                        else_branch.span.clone(),
                        "unsupported procedural else branch",
                    ));
                }
                let next_condition = match condition {
                    Some(ref parent) => Expr::value(
                        ValueOperator::And,
                        vec![parent.clone(), self.lower_expr(&stmt.condition)?],
                    ),
                    None => self.lower_expr(&stmt.condition)?,
                };
                self.lower_procedural_body(&stmt.then_branch, Some(next_condition), hold_on_false)
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
        condition: Option<&Expr>,
        hold_on_false: bool,
    ) -> LowerResult<()> {
        let target = expr_symbol(&stmt.target).ok_or_else(|| {
            Diagnostic::new(
                stmt.target.span.clone(),
                "expected assignment target symbol",
            )
        })?;
        let mut expr = self.lower_expr(&stmt.value)?;
        if hold_on_false && let Some(condition) = condition {
            expr = Expr::value(
                ValueOperator::Mux,
                vec![condition.clone(), expr, Expr::atom(target.clone())],
            );
        }
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr,
            delay: Expr::atom("0"),
        }));
        Ok(())
    }

    fn lower_continuous_assign(&mut self, assign: &AssignDecl) -> LowerResult<()> {
        let target = expr_symbol(&assign.target).ok_or_else(|| {
            Diagnostic::new(
                assign.target.span.clone(),
                "expected assignment target symbol",
            )
        })?;
        let expr = self.lower_expr(&assign.value)?;
        let delay = self.lower_delay(assign.delay.as_ref())?;
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr,
            delay,
        }));
        Ok(())
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
                if let Some(expr) = self.lower_tristate_ternary(
                    condition.as_ref(),
                    then_expr.as_ref(),
                    else_expr.as_ref(),
                )? {
                    return Ok(expr);
                }
                if self.is_z_expr(then_expr) || self.is_z_expr(else_expr) {
                    return Err(Diagnostic::new(
                        expr.span.clone(),
                        "high-Z ternary is not yet a contracted polarity-equivalent driver form",
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
        if self.is_z_expr(else_expr)
            && let Some(value) = self.tristate_drive_value(then_expr)
        {
            return Ok(Some(Expr::value(
                ValueOperator::BufIf1,
                vec![value, self.lower_expr(condition)?],
            )));
        }
        if self.is_z_expr(then_expr)
            && let Some(value) = self.tristate_drive_value(else_expr)
        {
            return Ok(Some(Expr::value(
                ValueOperator::BufIf0,
                vec![value, self.lower_expr(condition)?],
            )));
        }
        Ok(None)
    }

    fn tristate_drive_value(&mut self, expr: &SvExpr) -> Option<Expr> {
        match &expr.kind {
            ExprKind::Constant(ConstKind::Zero) => Some(Expr::atom("0")),
            ExprKind::Constant(ConstKind::One) => Some(Expr::atom("1")),
            ExprKind::Integer(value) if value == "0" => Some(Expr::atom("0")),
            ExprKind::Integer(value) if value == "1" => Some(Expr::atom("1")),
            ExprKind::Group(inner) => self.tristate_drive_value(inner),
            _ => None,
        }
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
            "nmos" | "pmos" | "rnmos" => Err(Diagnostic::new(
                call.span.clone(),
                format!("unsupported primitive {}", call.name),
            )),
            _ => Err(Diagnostic::new(
                call.span.clone(),
                "unsupported primitive for lowering",
            )),
        }
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
        let operator = ValueOperator::parse(&call.name).ok_or_else(|| {
            Diagnostic::new(call.span.clone(), "uncontracted bufif value operator")
        })?;
        let expr = Expr::value(
            operator,
            vec![self.lower_expr(value)?, self.lower_expr(control)?],
        );
        let delay = self.lower_delay(call.delay.as_ref())?;
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr,
            delay,
        }));
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
                if segments.len() == 1
                    && let Some(alias) = self.timing_aliases.get(&segments[0])
                {
                    return Ok(alias.clone());
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

    fn lower_delay(&mut self, delay: Option<&Delay>) -> LowerResult<Expr> {
        match delay {
            Some(delay) => self.lower_timing_expr_from_delay(delay),
            None => Ok(Expr::atom("0")),
        }
    }

    fn lower_timing_expr_from_delay(&mut self, delay: &Delay) -> LowerResult<Expr> {
        let Some(first) = delay.values.first() else {
            return Err(Diagnostic::new(
                delay.span.clone(),
                "delay tuple must contain a first entry",
            ));
        };
        let first = first.as_ref().ok_or_else(|| {
            Diagnostic::new(
                delay.span.clone(),
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
        match &expr.kind {
            ExprKind::Call { callee, args } => {
                let name = expr_symbol(callee).unwrap_or_else(|| render_call_callee(callee));
                match name.as_str() {
                    "R_pmos_ohm" => {
                        self.lower_timing_resistance_call(TimingOperator::Pmos, callee, args)
                    }
                    "R_nmos_ohm" => {
                        self.lower_timing_resistance_call(TimingOperator::Nmos, callee, args)
                    }
                    _ => self.lower_timing_expr(expr),
                }
            }
            ExprKind::Binary {
                op: BinaryOp::Mul,
                left,
                right,
            } => {
                if matches!(&left.kind, ExprKind::Call { .. }) {
                    return self.lower_timing_resistance(left);
                }
                if matches!(&right.kind, ExprKind::Call { .. }) {
                    return self.lower_timing_resistance(right);
                }
                if let Some(value) = multiply_unit_factor(left, right) {
                    return Ok(Expr::atom(value.to_string()));
                }
                self.lower_timing_expr(expr)
            }
            _ => self.lower_timing_expr(expr),
        }
    }

    fn lower_timing_resistance_call(
        &mut self,
        operator: TimingOperator,
        callee: &SvExpr,
        args: &[Option<SvExpr>],
    ) -> LowerResult<Expr> {
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
        Ok(Expr::timing(operator, vec![Expr::atom(value.to_string())]))
    }

    fn extract_unit_factor(&self, expr: &SvExpr) -> LowerResult<i64> {
        match &expr.kind {
            ExprKind::Binary {
                op: BinaryOp::Mul,
                left,
                right,
            } => multiply_unit_factor(left, right)
                .ok_or_else(|| Diagnostic::new(expr.span.clone(), "unsupported timing factor")),
            ExprKind::Integer(value) => value
                .parse::<i64>()
                .map_err(|_| Diagnostic::new(expr.span.clone(), "invalid integer factor")),
            _ => Err(Diagnostic::new(
                expr.span.clone(),
                "unsupported timing factor",
            )),
        }
    }
}

fn expr_symbol(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) => Some(segments.join("::")),
        ExprKind::Group(inner) => expr_symbol(inner),
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

fn multiply_unit_factor(left: &SvExpr, right: &SvExpr) -> Option<i64> {
    fn factor(expr: &SvExpr) -> Option<i64> {
        match &expr.kind {
            ExprKind::Integer(value) => value.parse::<i64>().ok(),
            ExprKind::Path(segments) if segments.len() == 1 && segments[0] == "L_unit" => Some(1),
            ExprKind::Binary {
                op: BinaryOp::Mul,
                left,
                right,
            } => factor(left).and_then(|left| factor(right).map(|right| left * right)),
            _ => None,
        }
    }
    factor(left).or_else(|| factor(right))
}

#[cfg(test)]
mod tests {
    use super::*;
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
                    "ff1".to_string(),
                    "(mux (or (and d clk_n ena) (and (not d) (not clk) (not ena_n)) r) (and d (not r)) ff1)"
                        .to_string(),
                    "0".to_string(),
                ),
                (
                    "ff2".to_string(),
                    "(mux (or (and ff1 clk) (and (not ff1) (not clk_n))) (not ff1) ff2)"
                        .to_string(),
                    "0".to_string(),
                ),
                (
                    "q_n".to_string(),
                    "(mux (or (and ff2 clk) (and (not ff2) (not clk_n))) ff2 q_n)".to_string(),
                    "0".to_string(),
                ),
                (
                    "q".to_string(),
                    "(not q_n)".to_string(),
                    "0".to_string(),
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
                (
                    "q".to_string(),
                    "(mux (or (not s_n) (not r_n)) (not s_n) q)".to_string(),
                    "0".to_string(),
                ),
                (
                    "q_n".to_string(),
                    "(mux (or (not s_n) (not r_n)) (not r_n) q_n)".to_string(),
                    "0".to_string(),
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
                    "0".to_string(),
                ),
                ("q_n".to_string(), "(not q)".to_string(), "0".to_string(),),
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
                ("in".to_string(), "(bufif0 1 pch_n)".to_string()),
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
                ("pad".to_string(), "(bufif1 0 ndrv)".to_string()),
                ("pad".to_string(), "(bufif0 1 pdrv_n)".to_string()),
                ("i_n".to_string(), "(not pad)".to_string()),
            ]
        );
    }

    #[test]
    fn lowers_tristate_assigns_with_repeated_drivers_in_source_order() {
        let lowered = lower_path("../sv-cells/sm83/cells/reg_pc_out_bit012.sv");
        let assignments = assignment_strings(&lowered);
        assert!(assignments.iter().any(|(target, expr, _)| {
            target == "y1" && expr == "(bufif1 0 (or (and in1 in2) (and in3 in4)))"
        }));
        let y4_assignments = assignments
            .iter()
            .filter(|(target, _, _)| target == "y4")
            .map(|(_, expr, _)| expr.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            y4_assignments,
            vec![
                "(bufif1 0 (and in7 in8))".to_string(),
                "(bufif1 0 in9)".to_string(),
            ]
        );
    }

    #[test]
    fn delay_tuples_select_exactly_the_first_entry() {
        for (delay, expected) in [("#(1)", "1"), ("#(1, 2)", "1"), ("#(1, 2, 3)", "1")] {
            let input = format!(
                "module sample(input logic a, output logic y); assign {delay} y = a; endmodule"
            );
            let lowered = lower_snippet(&input).unwrap();
            assert_eq!(assignment_strings(&lowered)[0].2, expected);
        }
        let lowered =
            lower_snippet("module sample(input logic a, output logic y); assign y = a; endmodule")
                .unwrap();
        assert_eq!(assignment_strings(&lowered)[0].2, "0");
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
    fn high_z_lowers_only_as_an_equality_operand() {
        let equality = lower_snippet(
            "module sample(input logic a, output logic y); assign y = a === 'z; endmodule",
        )
        .unwrap();
        assert_eq!(assignment_strings(&equality)[0].1, "(caseeq a z)");

        let direct =
            lower_snippet("module sample(input logic a, output logic y); assign y = 'z; endmodule")
                .unwrap_err();
        assert!(direct.message.contains("high-Z"));

        let unimplemented_tristate = lower_snippet(
            "module sample(input logic a, input logic s, output logic y); assign y = s ? a : 'z; endmodule",
        )
        .unwrap_err();
        assert!(unimplemented_tristate.message.contains("high-Z ternary"));
    }
}
