use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::parser::parse_file;
use std::collections::BTreeMap;
use std::path::Path;

pub type AnalyzeResult<T> = Result<T, Diagnostic>;

pub fn analyze_file(path: &Path, input: &str) -> AnalyzeResult<AnalysisReport> {
    let design = parse_file(path, input)?;
    Ok(analyze_design(&design))
}

pub fn analyze_design(design: &Design) -> AnalysisReport {
    AnalysisReport {
        modules: design.modules().map(analyze_module).collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisReport {
    pub modules: Vec<ModuleAnalysis>,
}

impl AnalysisReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "analyze summary: modules={}\n",
            self.modules.len()
        ));
        for module in &self.modules {
            out.push_str(&module.render());
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAnalysis {
    pub span: crate::diagnostic::Span,
    pub name: String,
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
}

impl ModuleAnalysis {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("module {}\n", self.name));
        out.push_str(&format!("  inputs: {}\n", join_list(&self.inputs)));
        out.push_str(&format!("  outputs: {}\n", join_list(&self.outputs)));
        out.push_str(&format!("  registers: {}\n", join_list(&self.registers)));
        out.push_str(&format!(
            "  symbols: parameters={} declarations={} localparams={} specparams={}\n",
            self.parameters.len(),
            self.declarations.len(),
            self.localparams.len(),
            self.specparams.len()
        ));
        out.push_str(&format!(
            "  assignments: continuous={} initial={} procedural={}\n",
            self.continuous_assignments.len(),
            self.initial_assignments.len(),
            self.procedural_assignments.len()
        ));
        out.push_str(&format!(
            "  primitives: {}\n",
            render_counts(
                self.primitive_calls
                    .iter()
                    .fold(BTreeMap::<String, usize>::new(), |mut acc, call| {
                        *acc.entry(call.name.clone()).or_insert(0) += 1;
                        acc
                    })
                    .iter()
                    .map(|(name, count)| format!("{}={}", name, count))
                    .collect::<Vec<_>>()
            )
        ));
        out.push_str(&format!(
            "  instantiations: {}\n",
            self.instantiations.len()
        ));
        out.push_str(&format!("  specify_paths: {}\n", self.specify_paths.len()));
        out.push_str(&format!(
            "  timing_aliases: {}\n",
            self.timing_aliases.len()
        ));
        if !self.continuous_assignments.is_empty() {
            out.push_str("  continuous detail:\n");
            for item in &self.continuous_assignments {
                out.push_str(&format!("    {}\n", item.render()));
            }
        }
        if !self.procedural_assignments.is_empty() {
            out.push_str("  procedural detail:\n");
            for item in &self.procedural_assignments {
                out.push_str(&format!("    {}\n", item.render()));
            }
        }
        if !self.initial_assignments.is_empty() {
            out.push_str("  initial detail:\n");
            for item in &self.initial_assignments {
                out.push_str(&format!("    {}\n", item.render()));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAnalysis {
    pub direction: Direction,
    pub modifiers: Vec<String>,
    pub declared: String,
    pub is_input: bool,
    pub is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclAnalysis {
    pub kind: DeclKind,
    pub ty: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueAnalysis {
    pub kind: ParamKind,
    pub ty: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingAlias {
    pub kind: ParamKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentAnalysis {
    pub target: String,
    pub value: String,
    pub delay: Option<String>,
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
    pub name: String,
    pub strength: Option<Vec<String>>,
    pub delay: Option<String>,
    pub args: Vec<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstantiationAnalysis {
    pub module: String,
    pub instance: String,
    pub parameters: Vec<String>,
    pub connections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecPathAnalysis {
    pub controls: Vec<String>,
    pub target: String,
    pub delays: Vec<Option<String>>,
}

fn analyze_module(module: &Module) -> ModuleAnalysis {
    let mut analysis = ModuleAnalysis {
        span: module.span.clone(),
        name: module.name.clone(),
        ports: BTreeMap::new(),
        parameters: BTreeMap::new(),
        declarations: BTreeMap::new(),
        localparams: BTreeMap::new(),
        specparams: BTreeMap::new(),
        timing_aliases: BTreeMap::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        registers: Vec::new(),
        continuous_assignments: Vec::new(),
        initial_assignments: Vec::new(),
        procedural_assignments: Vec::new(),
        primitive_calls: Vec::new(),
        instantiations: Vec::new(),
        specify_paths: Vec::new(),
    };

    for param in &module.parameters {
        let value = render_expr(&param.value);
        let entry = ValueAnalysis {
            kind: param.kind,
            ty: param.ty.clone(),
            value: value.clone(),
        };
        match param.kind {
            ParamKind::Parameter => {
                analysis.parameters.insert(param.name.clone(), entry);
            }
            ParamKind::Localparam => {
                analysis
                    .localparams
                    .insert(param.name.clone(), entry.clone());
                analysis.timing_aliases.insert(
                    param.name.clone(),
                    TimingAlias {
                        kind: param.kind,
                        value,
                    },
                );
            }
            ParamKind::Specparam => {
                analysis
                    .specparams
                    .insert(param.name.clone(), entry.clone());
                analysis.timing_aliases.insert(
                    param.name.clone(),
                    TimingAlias {
                        kind: param.kind,
                        value,
                    },
                );
            }
        }
    }

    for port in &module.ports {
        for name in &port.names {
            analysis.ports.insert(
                name.clone(),
                PortAnalysis {
                    direction: port.direction,
                    modifiers: port.modifiers.clone(),
                    declared: name.clone(),
                    is_input: matches!(port.direction, Direction::Input),
                    is_output: matches!(port.direction, Direction::Output),
                },
            );
            match port.direction {
                Direction::Input => {
                    push_unique(&mut analysis.inputs, name.clone());
                }
                Direction::Output => {
                    push_unique(&mut analysis.outputs, name.clone());
                }
                Direction::Inout => {}
            }
        }
    }

    for item in &module.items {
        analyze_item(item, &mut analysis, ProceduralContext::Root);
    }

    analysis
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

fn analyze_item(item: &Item, analysis: &mut ModuleAnalysis, context: ProceduralContext) {
    match &item.kind {
        ItemKind::Import(_) | ItemKind::Empty => {}
        ItemKind::Decl(decl) => analyze_decl(decl, analysis),
        ItemKind::Initial(stmt) => {
            analyze_initial(stmt, analysis);
        }
        ItemKind::ProcAssign(stmt) => {
            analyze_assignment(stmt, analysis, context);
        }
        ItemKind::AlwaysLatch(always) => {
            if let Some(condition) = &always.condition {
                collect_expr_reads(condition, analysis);
            }
            analyze_item(
                &always.body,
                analysis,
                ProceduralContext::Always {
                    state: true,
                    source: ProceduralSource::AlwaysLatch,
                },
            );
        }
        ItemKind::Always(always) => {
            if let Some(sensitivity) = &always.sensitivity
                && sensitivity_is_stateful(sensitivity, always.kind)
            {
                analyze_item(
                    &always.body,
                    analysis,
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
                analysis,
                ProceduralContext::Always {
                    state: false,
                    source: ProceduralSource::Always,
                },
            );
        }
        ItemKind::Assign(assign) => {
            analyze_continuous_assign(assign, analysis);
        }
        ItemKind::Primitive(call) => {
            analyze_primitive(call, analysis);
        }
        ItemKind::Instantiation(instantiation) => {
            analyze_instantiation(instantiation, analysis);
        }
        ItemKind::Specify(specify) => {
            analyze_specify(specify, analysis);
        }
        ItemKind::Generate(block) | ItemKind::Block(block) => {
            for child in &block.items {
                analyze_item(child, analysis, context);
            }
        }
        ItemKind::If(stmt) => {
            collect_expr_reads(&stmt.condition, analysis);
            analyze_item(&stmt.then_branch, analysis, context);
            if let Some(else_branch) = &stmt.else_branch {
                analyze_item(else_branch, analysis, context);
            }
        }
    }
}

fn analyze_decl(decl: &Decl, analysis: &mut ModuleAnalysis) {
    let value = decl.value.as_ref().map(render_expr);
    match decl.kind {
        DeclKind::Logic | DeclKind::Tri | DeclKind::Wire => {
            for name in &decl.names {
                analysis.declarations.insert(
                    name.clone(),
                    DeclAnalysis {
                        kind: decl.kind,
                        ty: decl.ty.clone(),
                        value: value.clone(),
                    },
                );
            }
        }
        DeclKind::Parameter => {
            for name in &decl.names {
                if let Some(value) = &value {
                    analysis.parameters.insert(
                        name.clone(),
                        ValueAnalysis {
                            kind: ParamKind::Parameter,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                        },
                    );
                }
            }
        }
        DeclKind::Localparam => {
            for name in &decl.names {
                if let Some(value) = &value {
                    analysis.localparams.insert(
                        name.clone(),
                        ValueAnalysis {
                            kind: ParamKind::Localparam,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                        },
                    );
                    analysis.timing_aliases.insert(
                        name.clone(),
                        TimingAlias {
                            kind: ParamKind::Localparam,
                            value: value.clone(),
                        },
                    );
                }
            }
        }
        DeclKind::Specparam => {
            for name in &decl.names {
                if let Some(value) = &value {
                    analysis.specparams.insert(
                        name.clone(),
                        ValueAnalysis {
                            kind: ParamKind::Specparam,
                            ty: decl.ty.clone(),
                            value: value.clone(),
                        },
                    );
                    analysis.timing_aliases.insert(
                        name.clone(),
                        TimingAlias {
                            kind: ParamKind::Specparam,
                            value: value.clone(),
                        },
                    );
                }
            }
        }
    }
}

fn analyze_initial(stmt: &AssignStmt, analysis: &mut ModuleAnalysis) {
    let target = render_expr(&stmt.target);
    let value = render_expr(&stmt.value);
    collect_expr_reads(&stmt.value, analysis);
    mark_writes(&stmt.target, analysis);
    if let Some(name) = expr_symbol(&stmt.target) {
        register_name(&name, analysis);
    }
    analysis.initial_assignments.push(AssignmentAnalysis {
        target,
        value,
        delay: None,
        kind: AssignmentKind::Initial,
    });
}

fn analyze_assignment(
    stmt: &AssignStmt,
    analysis: &mut ModuleAnalysis,
    context: ProceduralContext,
) {
    let target = render_expr(&stmt.target);
    let value = render_expr(&stmt.value);
    collect_expr_reads(&stmt.value, analysis);
    mark_writes(&stmt.target, analysis);
    let kind = AssignmentKind::Procedural {
        state: context.is_state(),
        source: context.source().unwrap_or(ProceduralSource::Always),
    };
    analysis.procedural_assignments.push(AssignmentAnalysis {
        target,
        value,
        delay: None,
        kind,
    });
    if context.is_state()
        && let Some(name) = expr_symbol(&stmt.target)
    {
        register_name(&name, analysis);
    }
}

fn analyze_continuous_assign(assign: &AssignDecl, analysis: &mut ModuleAnalysis) {
    let target = render_expr(&assign.target);
    let value = render_expr(&assign.value);
    if let Some(delay) = &assign.delay {
        collect_optional_exprs(&delay.values, analysis);
    }
    collect_expr_reads(&assign.value, analysis);
    mark_writes(&assign.target, analysis);
    analysis.continuous_assignments.push(AssignmentAnalysis {
        target,
        value,
        delay: assign.delay.as_ref().map(render_delay),
        kind: AssignmentKind::Continuous,
    });
}

fn analyze_primitive(call: &PrimitiveCall, analysis: &mut ModuleAnalysis) {
    if let Some(delay) = &call.delay {
        collect_optional_exprs(&delay.values, analysis);
    }
    if let Some(strength) = &call.strength {
        // Strength groups are preserved as raw identifiers for deterministic summaries.
        let _ = strength;
    }
    let mut args = Vec::with_capacity(call.args.len());
    for (index, arg) in call.args.iter().enumerate() {
        match arg {
            Some(expr) => {
                if index == 0 {
                    mark_writes(expr, analysis);
                } else {
                    collect_expr_reads(expr, analysis);
                }
                args.push(Some(render_expr(expr)));
            }
            None => args.push(None),
        }
    }
    analysis.primitive_calls.push(PrimitiveAnalysis {
        name: call.name.clone(),
        strength: call.strength.as_ref().map(|s| s.values.clone()),
        delay: call.delay.as_ref().map(render_delay),
        args,
    });
}

fn analyze_instantiation(inst: &Instantiation, analysis: &mut ModuleAnalysis) {
    let mut parameters = Vec::with_capacity(inst.parameters.len());
    for override_item in &inst.parameters {
        match &override_item.kind {
            ParamOverrideKind::Named { value, .. } => {
                collect_expr_reads(value, analysis);
                parameters.push(render_expr(value));
            }
            ParamOverrideKind::Positional(Some(value)) => {
                collect_expr_reads(value, analysis);
                parameters.push(render_expr(value));
            }
            ParamOverrideKind::Positional(None) => parameters.push("_".to_string()),
        }
    }
    let mut connections = Vec::with_capacity(inst.connections.len());
    for connection in &inst.connections {
        match &connection.kind {
            ConnectionKind::Named { value, .. } => {
                collect_expr_reads(value, analysis);
                connections.push(render_expr(value));
            }
            ConnectionKind::Positional(value) => {
                collect_expr_reads(value, analysis);
                connections.push(render_expr(value));
            }
        }
    }
    analysis.instantiations.push(InstantiationAnalysis {
        module: inst.module.clone(),
        instance: inst.instance.clone(),
        parameters,
        connections,
    });
}

fn analyze_specify(specify: &SpecifyBlock, analysis: &mut ModuleAnalysis) {
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
                    analysis,
                );
            }
            SpecifyItem::Path(path) => {
                collect_expr_reads(&path.target, analysis);
                for control in &path.controls {
                    collect_expr_reads(control, analysis);
                }
                for expr in path.delays.iter().flatten() {
                    collect_expr_reads(expr, analysis);
                }
                analysis.specify_paths.push(SpecPathAnalysis {
                    controls: path.controls.iter().map(render_expr).collect(),
                    target: render_expr(&path.target),
                    delays: path
                        .delays
                        .iter()
                        .map(|item| item.as_ref().map(render_expr))
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

fn collect_expr_reads(expr: &Expr, analysis: &mut ModuleAnalysis) {
    match &expr.kind {
        ExprKind::Path(segments) => {
            if let Some(name) = segments.last() {
                mark_read(name, analysis);
            }
        }
        ExprKind::Group(inner) => collect_expr_reads(inner, analysis),
        ExprKind::Unary { expr, .. } => collect_expr_reads(expr, analysis),
        ExprKind::Binary { left, right, .. } => {
            collect_expr_reads(left, analysis);
            collect_expr_reads(right, analysis);
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_expr_reads(condition, analysis);
            collect_expr_reads(then_expr, analysis);
            collect_expr_reads(else_expr, analysis);
        }
        ExprKind::Call { callee, args } => {
            collect_expr_reads(callee, analysis);
            for expr in args.iter().flatten() {
                collect_expr_reads(expr, analysis);
            }
        }
        ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {}
    }
}

fn collect_optional_exprs(exprs: &[Option<Expr>], analysis: &mut ModuleAnalysis) {
    for expr in exprs.iter().flatten() {
        collect_expr_reads(expr, analysis);
    }
}

fn mark_writes(expr: &Expr, analysis: &mut ModuleAnalysis) {
    if let Some(name) = expr_symbol(expr) {
        if let Some(port) = analysis.ports.get_mut(&name)
            && matches!(port.direction, Direction::Inout)
        {
            port.is_output = true;
            push_unique(&mut analysis.outputs, name.clone());
        }
        if analysis.declarations.contains_key(&name)
            || analysis.localparams.contains_key(&name)
            || analysis.specparams.contains_key(&name)
        {
            register_name(&name, analysis);
        }
    }
}

fn mark_read(name: &str, analysis: &mut ModuleAnalysis) {
    if let Some(port) = analysis.ports.get_mut(name)
        && matches!(port.direction, Direction::Inout)
    {
        port.is_input = true;
        push_unique(&mut analysis.inputs, name.to_string());
    }
}

fn register_name(name: &str, analysis: &mut ModuleAnalysis) {
    push_unique(&mut analysis.registers, name.to_string());
}

fn expr_symbol(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) => Some(segments.join("::")),
        ExprKind::Group(inner) => expr_symbol(inner),
        _ => None,
    }
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

fn render_counts(items: Vec<String>) -> String {
    if items.is_empty() {
        "<none>".to_string()
    } else {
        items.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn analyze_path(path: &str) -> AnalysisReport {
        let path = Path::new(path);
        let input = fs::read_to_string(path).unwrap();
        analyze_file(path, &input).unwrap()
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
}
