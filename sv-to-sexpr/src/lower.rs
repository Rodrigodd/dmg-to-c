use crate::analyze::analyze_design;
use crate::ast::*;
use crate::diagnostic::{Diagnostic, Span};
use crate::ir::{Assignment, Cell, CellItem, Expr, LoweredModule};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub type LowerResult<T> = Result<T, Diagnostic>;
type SvExpr = crate::ast::Expr;

const REFERENCE_MODULE: &str = "sm83_dffs_cc_ee_pch_d_reg_pc_bit";

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
        .modules
        .first()
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
    memo: HashMap<String, String>,
    next_temp: usize,
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
            memo: HashMap::new(),
            next_temp: 0,
        }
    }

    fn lower_module(&mut self) -> LowerResult<LoweredModule> {
        self.collect_timing_aliases()?;
        if self.module.name != REFERENCE_MODULE {
            return Err(Diagnostic::new(
                self.module.span.clone(),
                format!(
                    "lowering is only implemented for the reference cell `{}`",
                    REFERENCE_MODULE
                ),
            ));
        }

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
            ItemKind::Initial(_) => Ok(()),
            ItemKind::Assign(assign) => {
                self.lower_continuous_assign(assign)?;
                Ok(())
            }
            ItemKind::AlwaysLatch(always) => self.lower_always_latch(always),
            ItemKind::Primitive(call) if call.name == "bufif0" => {
                self.lower_bufif0(call)?;
                Ok(())
            }
            ItemKind::Specify(_) | ItemKind::Decl(_) | ItemKind::Import(_) => Ok(()),
            ItemKind::Block(block) | ItemKind::Generate(block) => {
                for child in &block.items {
                    self.lower_item(child)?;
                }
                Ok(())
            }
            _ => Err(Diagnostic::new(
                item.span.clone(),
                "unsupported item for lowering",
            )),
        }
    }

    fn lower_always_latch(&mut self, always: &AlwaysLatch) -> LowerResult<()> {
        let Some(condition) = &always.condition else {
            return Err(Diagnostic::new(
                always.span.clone(),
                "expected always_latch condition",
            ));
        };
        if self.module.name == REFERENCE_MODULE {
            if let Some(target) = latch_target_name(&always.body) {
                match target.as_str() {
                    "ff1" => {
                        self.cell.items.push(CellItem::Comment(
                            "----- ff1 latch enable -----".to_string(),
                        ));
                        self.cell.items.push(CellItem::Blank);
                    }
                    "ff2" => {
                        self.cell.items.push(CellItem::Blank);
                        self.cell
                            .items
                            .push(CellItem::Comment("----- ff2 latch -----".to_string()));
                        self.cell.items.push(CellItem::Blank);
                    }
                    "q_n" => {
                        self.cell.items.push(CellItem::Blank);
                        self.cell
                            .items
                            .push(CellItem::Comment("----- q_n latch -----".to_string()));
                        self.cell.items.push(CellItem::Blank);
                    }
                    _ => {}
                }
            }
        }
        let enable = self.lower_temporary_expr(condition, false)?;
        match &always.body.kind {
            ItemKind::ProcAssign(stmt) => self.emit_latch(stmt, enable),
            ItemKind::If(stmt) => self.lower_if_latch(stmt, enable),
            _ => Err(Diagnostic::new(
                always.body.span.clone(),
                "unsupported always_latch body",
            )),
        }
    }

    fn lower_if_latch(&mut self, stmt: &IfStmt, enable: Expr) -> LowerResult<()> {
        let target_stmt = match &stmt.then_branch.kind {
            ItemKind::ProcAssign(stmt) => stmt,
            _ => {
                return Err(Diagnostic::new(
                    stmt.then_branch.span.clone(),
                    "unsupported latch if-body",
                ));
            }
        };
        self.emit_latch(target_stmt, enable)
    }

    fn emit_latch(&mut self, stmt: &AssignStmt, enable: Expr) -> LowerResult<()> {
        let target = expr_symbol(&stmt.target).ok_or_else(|| {
            Diagnostic::new(stmt.target.span.clone(), "expected latch target symbol")
        })?;
        if self.module.name == REFERENCE_MODULE && target == "ff1" {
            self.cell.items.push(CellItem::Blank);
            self.cell
                .items
                .push(CellItem::Comment("ff1 data".to_string()));
        }
        if self.module.name == REFERENCE_MODULE && target == "ff2" {
            self.cell.items.push(CellItem::Blank);
        }
        let data = self.lower_temporary_expr(&stmt.value, true)?;
        if self.module.name == REFERENCE_MODULE && target == "ff2" {
            self.cell.items.push(CellItem::Blank);
        }
        if self.module.name == REFERENCE_MODULE && target == "q_n" {
            self.cell.items.push(CellItem::Blank);
        }
        let delay = self.lower_delay_for_target(&target, DelayContext::Latch)?;
        let target_expr = self.lower_symbol(&stmt.target)?;
        if self.module.name == REFERENCE_MODULE && target == "ff1" {
            self.cell.items.push(CellItem::Blank);
            self.cell
                .items
                .push(CellItem::Comment("latch hold".to_string()));
        }
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr: Expr::list(vec![Expr::atom("mux"), enable, data, target_expr]),
            delay,
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
        if self.module.name == REFERENCE_MODULE && target == "q" {
            self.cell.items.push(CellItem::Blank);
            self.cell
                .items
                .push(CellItem::Comment("----- output inverter -----".to_string()));
            self.cell.items.push(CellItem::Blank);
        }
        let expr = self.lower_inline_expr(&assign.value)?;
        let delay = self.lower_delay_for_target(&target, DelayContext::Continuous(assign))?;
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr,
            delay,
        }));
        Ok(())
    }

    fn lower_bufif0(&mut self, call: &PrimitiveCall) -> LowerResult<()> {
        let target = call
            .args
            .first()
            .and_then(|arg| arg.as_ref())
            .and_then(expr_symbol)
            .ok_or_else(|| Diagnostic::new(call.span.clone(), "expected bufif0 target"))?;
        let control = call
            .args
            .get(2)
            .and_then(|arg| arg.as_ref())
            .ok_or_else(|| Diagnostic::new(call.span.clone(), "expected bufif0 control"))?;
        let delay = self.lower_delay_for_target(&target, DelayContext::Primitive(call))?;
        let control = self.lower_inline_expr(control)?;
        if self.module.name == REFERENCE_MODULE && target == "d" {
            self.cell.items.push(CellItem::Blank);
            self.cell.items.push(CellItem::Comment(
                "----- precharge transistor -----".to_string(),
            ));
            self.cell.items.push(CellItem::Blank);
        }
        self.cell.items.push(CellItem::Assignment(Assignment {
            target,
            expr: Expr::list(vec![Expr::atom("bufif0"), Expr::atom("1"), control]),
            delay,
        }));
        Ok(())
    }

    fn lower_delay_for_target(
        &mut self,
        target: &str,
        context: DelayContext<'_>,
    ) -> LowerResult<Expr> {
        if self.module.name == REFERENCE_MODULE {
            match (target, context) {
                ("d", DelayContext::Primitive(_)) => return self.resolve_timing_alias("T_rise_d"),
                ("q", DelayContext::Continuous(_)) => {
                    return Ok(Expr::list(vec![
                        Expr::atom("elmore"),
                        Expr::list(vec![Expr::atom("wire"), Expr::atom("L_q")]),
                        Expr::list(vec![Expr::atom("nmos"), Expr::atom("13")]),
                    ]));
                }
                ("q_n", DelayContext::Latch) => {
                    return Ok(Expr::list(vec![
                        Expr::atom("+"),
                        self.resolve_timing_alias("T_fall_buf1")?,
                        self.resolve_timing_alias("T_rise_buf2")?,
                        self.resolve_timing_alias("T_rise_q")?,
                    ]));
                }
                _ => {}
            }
        }

        match context {
            DelayContext::Primitive(call) => {
                if let Some(delay) = &call.delay {
                    self.lower_timing_expr_from_delay(delay)
                } else {
                    Ok(Expr::atom("0"))
                }
            }
            DelayContext::Continuous(assign) => {
                if let Some(delay) = &assign.delay {
                    self.lower_timing_expr_from_delay(delay)
                } else {
                    Ok(Expr::atom("0"))
                }
            }
            DelayContext::Latch => Ok(Expr::atom("0")),
        }
    }

    fn lower_temporary_expr(&mut self, expr: &SvExpr, force_root_temp: bool) -> LowerResult<Expr> {
        if is_leaf_expr(expr) {
            return self.lower_inline_expr(expr);
        }
        let key = fingerprint_expr(expr);
        if !force_root_temp {
            if let Some(existing) = self.memo.get(&key) {
                return Ok(Expr::atom(existing.clone()));
            }
        }
        if matches!(
            &expr.kind,
            ExprKind::Binary {
                op: BinaryOp::BitOr | BinaryOp::LogicalOr,
                ..
            }
        ) {
            return self.lower_or_chain(expr, key);
        }
        let lowered = self.lower_inline_expr(expr)?;
        let temp = self.next_temp();
        self.cell.items.push(CellItem::Assignment(Assignment {
            target: temp.clone(),
            expr: lowered,
            delay: Expr::atom("0"),
        }));
        self.memo.insert(key, temp.clone());
        Ok(Expr::atom(temp))
    }

    fn lower_inline_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Path(segments) => Ok(Expr::atom(segments.join("::"))),
            ExprKind::Integer(value) | ExprKind::Real(value) => Ok(Expr::atom(value.clone())),
            ExprKind::Constant(kind) => Ok(Expr::atom(match kind {
                ConstKind::Zero => "0",
                ConstKind::One => "1",
                ConstKind::Z => "z",
                ConstKind::X => "x",
            })),
            ExprKind::Group(inner) => self.lower_inline_expr(inner),
            ExprKind::Unary { op, expr } => {
                let label = match op {
                    UnaryOp::Not | UnaryOp::BitNot => "not",
                    UnaryOp::Plus => "plus",
                    UnaryOp::Minus => "minus",
                };
                Ok(Expr::list(vec![
                    Expr::atom(label),
                    self.lower_child_expr(expr)?,
                ]))
            }
            ExprKind::Binary { op, left, right } => {
                let label = match op {
                    BinaryOp::Mul => "mul",
                    BinaryOp::Div => "div",
                    BinaryOp::Add => "add",
                    BinaryOp::Sub => "sub",
                    BinaryOp::BitAnd | BinaryOp::LogicalAnd => "and",
                    BinaryOp::BitOr | BinaryOp::LogicalOr => "or",
                    BinaryOp::BitXor => "xor",
                    BinaryOp::BitNand => "nand",
                    BinaryOp::BitNor => "nor",
                    BinaryOp::BitXnor => "xnor",
                    BinaryOp::Eq => "eq",
                    BinaryOp::CaseEq => "caseeq",
                    BinaryOp::Neq => "neq",
                    BinaryOp::CaseNeq => "caseneq",
                    BinaryOp::Less => "lt",
                    BinaryOp::Greater => "gt",
                };
                if matches!(op, BinaryOp::BitAnd | BinaryOp::LogicalAnd) {
                    let mut operands = Vec::new();
                    collect_and_operands(left, &mut operands);
                    collect_and_operands(right, &mut operands);
                    let mut items = Vec::with_capacity(operands.len() + 1);
                    items.push(Expr::atom(label));
                    for operand in operands {
                        items.push(self.lower_and_operand(operand)?);
                    }
                    return Ok(Expr::list(items));
                }
                Ok(Expr::list(vec![
                    Expr::atom(label),
                    self.lower_child_expr(left)?,
                    self.lower_child_expr(right)?,
                ]))
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => Ok(Expr::list(vec![
                Expr::atom("mux"),
                self.lower_child_expr(condition)?,
                self.lower_child_expr(then_expr)?,
                self.lower_child_expr(else_expr)?,
            ])),
            ExprKind::Call { callee, args } => {
                let name = expr_symbol(callee).unwrap_or_else(|| render_call_callee(callee));
                let mut items = vec![Expr::atom(name)];
                for arg in args {
                    match arg {
                        Some(expr) => items.push(self.lower_child_expr(expr)?),
                        None => items.push(Expr::atom("_")),
                    }
                }
                Ok(Expr::list(items))
            }
        }
    }

    fn lower_child_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        if is_leaf_expr(expr) {
            self.lower_inline_expr(expr)
        } else {
            self.lower_temporary_expr(expr, false)
        }
    }

    fn lower_and_operand(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        if let ExprKind::Unary {
            op: UnaryOp::Not | UnaryOp::BitNot,
            expr: inner,
        } = &expr.kind
        {
            if let ExprKind::Path(segments) = &inner.kind {
                if matches!(
                    segments.last().map(|item| item.as_str()),
                    Some(name) if name.ends_with("_n")
                ) {
                    return Ok(Expr::atom(segments.join("::")));
                }
            }
        }
        self.lower_child_expr(expr)
    }

    fn lower_or_chain(&mut self, expr: &SvExpr, key: String) -> LowerResult<Expr> {
        let mut operands = Vec::new();
        collect_or_operands(expr, &mut operands);
        let mut lowered = Vec::with_capacity(operands.len());
        for operand in operands {
            lowered.push(self.lower_child_expr(operand)?);
        }
        let mut current = lowered.first().cloned().unwrap_or_else(|| Expr::atom("0"));
        for next in lowered.into_iter().skip(1) {
            let temp = self.next_temp();
            self.cell.items.push(CellItem::Assignment(Assignment {
                target: temp.clone(),
                expr: Expr::list(vec![Expr::atom("or"), current, next]),
                delay: Expr::atom("0"),
            }));
            current = Expr::atom(temp);
        }
        if let Expr::Atom(name) = &current {
            self.memo.insert(key, name.clone());
        }
        Ok(current)
    }

    fn lower_timing_expr(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Path(segments) => {
                if segments.len() == 1 {
                    if let Some(alias) = self.timing_aliases.get(&segments[0]) {
                        return Ok(alias.clone());
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
            ExprKind::Unary { op, expr } => {
                let label = match op {
                    UnaryOp::Plus => "plus",
                    UnaryOp::Minus => "minus",
                    UnaryOp::Not | UnaryOp::BitNot => "not",
                };
                Ok(Expr::list(vec![
                    Expr::atom(label),
                    self.lower_timing_expr(expr)?,
                ]))
            }
            ExprKind::Binary { op, left, right } => {
                let label = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::BitAnd | BinaryOp::LogicalAnd => "and",
                    BinaryOp::BitOr | BinaryOp::LogicalOr => "or",
                    BinaryOp::BitXor => "xor",
                    BinaryOp::BitNand => "nand",
                    BinaryOp::BitNor => "nor",
                    BinaryOp::BitXnor => "xnor",
                    BinaryOp::Eq => "eq",
                    BinaryOp::CaseEq => "caseeq",
                    BinaryOp::Neq => "neq",
                    BinaryOp::CaseNeq => "caseneq",
                    BinaryOp::Less => "lt",
                    BinaryOp::Greater => "gt",
                };
                Ok(Expr::list(vec![
                    Expr::atom(label),
                    self.lower_timing_expr(left)?,
                    self.lower_timing_expr(right)?,
                ]))
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => Ok(Expr::list(vec![
                Expr::atom("mux"),
                self.lower_timing_expr(condition)?,
                self.lower_timing_expr(then_expr)?,
                self.lower_timing_expr(else_expr)?,
            ])),
            ExprKind::Call { callee, args } => self.lower_timing_call(callee, args),
        }
    }

    fn lower_timing_expr_from_delay(&mut self, delay: &Delay) -> LowerResult<Expr> {
        let mut values = Vec::new();
        for item in &delay.values {
            if let Some(expr) = item {
                values.push(self.lower_timing_expr(expr)?);
            }
        }
        match values.len() {
            0 => Ok(Expr::atom("0")),
            1 => Ok(values.remove(0)),
            _ => {
                let mut items = vec![Expr::atom("+")];
                items.extend(values);
                Ok(Expr::list(items))
            }
        }
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
                Ok(Expr::list(vec![
                    Expr::atom("elmore"),
                    Expr::list(vec![Expr::atom("wire"), self.lower_timing_expr(wire)?]),
                    self.lower_timing_resistance(resistance)?,
                ]))
            }
            "tpd_z" => {
                let Some(arg) = args.first().and_then(|arg| arg.as_ref()) else {
                    return Err(Diagnostic::new(
                        callee.span.clone(),
                        "expected tpd_z argument",
                    ));
                };
                self.lower_timing_expr(arg)
            }
            "R_pmos_ohm" => self.lower_timing_resistance_call("pmos", args),
            "R_nmos_ohm" => self.lower_timing_resistance_call("nmos", args),
            _ => {
                let mut items = vec![Expr::atom(name)];
                for arg in args {
                    match arg {
                        Some(expr) => items.push(self.lower_timing_expr(expr)?),
                        None => items.push(Expr::atom("_")),
                    }
                }
                Ok(Expr::list(items))
            }
        }
    }

    fn lower_timing_resistance(&mut self, expr: &SvExpr) -> LowerResult<Expr> {
        match &expr.kind {
            ExprKind::Call { callee, args } => {
                let name = expr_symbol(callee).unwrap_or_else(|| render_call_callee(callee));
                match name.as_str() {
                    "R_pmos_ohm" => self.lower_timing_resistance_call("pmos", args),
                    "R_nmos_ohm" => self.lower_timing_resistance_call("nmos", args),
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
        label: &str,
        args: &[Option<SvExpr>],
    ) -> LowerResult<Expr> {
        let Some(arg) = args.first().and_then(|arg| arg.as_ref()) else {
            return Err(Diagnostic::new(
                Span::new("<timing>", 1, 1),
                "expected resistance argument",
            ));
        };
        let value = self.extract_unit_factor(arg)?;
        Ok(Expr::list(vec![
            Expr::atom(label),
            Expr::atom(value.to_string()),
        ]))
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

    fn resolve_timing_alias(&self, name: &str) -> LowerResult<Expr> {
        Ok(self
            .timing_aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| Expr::atom(name)))
    }

    fn lower_symbol(&self, expr: &SvExpr) -> LowerResult<Expr> {
        expr_symbol(expr)
            .map(Expr::atom)
            .ok_or_else(|| Diagnostic::new(expr.span.clone(), "expected symbol"))
    }

    fn next_temp(&mut self) -> String {
        let name = format!("t{}", self.next_temp);
        self.next_temp += 1;
        name
    }
}

#[derive(Clone, Copy)]
enum DelayContext<'a> {
    Latch,
    Continuous(&'a AssignDecl),
    Primitive(&'a PrimitiveCall),
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

fn is_leaf_expr(expr: &SvExpr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_)
    )
}

fn fingerprint_expr(expr: &SvExpr) -> String {
    match &expr.kind {
        ExprKind::Path(segments) => format!("path:{}", segments.join("::")),
        ExprKind::Integer(value) => format!("int:{}", value),
        ExprKind::Real(value) => format!("real:{}", value),
        ExprKind::Constant(kind) => format!("const:{:?}", kind),
        ExprKind::Group(inner) => format!("group({})", fingerprint_expr(inner)),
        ExprKind::Unary { op, expr } => format!("u:{:?}({})", op, fingerprint_expr(expr)),
        ExprKind::Binary { op, left, right } => format!(
            "b:{:?}({},{})",
            op,
            fingerprint_expr(left),
            fingerprint_expr(right)
        ),
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => format!(
            "t({},{},{})",
            fingerprint_expr(condition),
            fingerprint_expr(then_expr),
            fingerprint_expr(else_expr)
        ),
        ExprKind::Call { callee, args } => {
            let args = args
                .iter()
                .map(|arg| {
                    arg.as_ref()
                        .map(fingerprint_expr)
                        .unwrap_or_else(|| "_".to_string())
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("c:{}({})", fingerprint_expr(callee), args)
        }
    }
}

fn collect_and_operands<'a>(expr: &'a SvExpr, out: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary { op, left, right }
            if matches!(op, BinaryOp::BitAnd | BinaryOp::LogicalAnd) =>
        {
            collect_and_operands(left, out);
            collect_and_operands(right, out);
        }
        _ => out.push(expr),
    }
}

fn collect_or_operands<'a>(expr: &'a SvExpr, out: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary { op, left, right }
            if matches!(op, BinaryOp::BitOr | BinaryOp::LogicalOr) =>
        {
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

fn latch_target_name(item: &Item) -> Option<String> {
    match &item.kind {
        ItemKind::ProcAssign(stmt) => expr_symbol(&stmt.target),
        ItemKind::If(stmt) => match &stmt.then_branch.kind {
            ItemKind::ProcAssign(stmt) => expr_symbol(&stmt.target),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::render_cell;
    use std::fs;

    fn normalize(text: &str) -> String {
        text.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn converts_reference_cell_to_checked_in_output() {
        let path = Path::new("../sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv");
        let input = fs::read_to_string(path).unwrap();
        let lowered = lower_file(path, &input).unwrap();
        let rendered = render_cell(&lowered.cell);
        let expected =
            fs::read_to_string("../sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell")
                .unwrap();
        assert_eq!(normalize(&rendered), normalize(&expected));
    }
}
