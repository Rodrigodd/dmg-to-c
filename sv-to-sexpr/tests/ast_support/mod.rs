use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use sv_to_sexpr::ast::*;
use sv_to_sexpr::diagnostic::Span;

#[derive(Debug, Default)]
pub struct AstCoverage {
    records: BTreeMap<String, CoverageRecord>,
}

#[derive(Debug, Default)]
struct CoverageRecord {
    count: usize,
    files: BTreeSet<String>,
}

impl AstCoverage {
    pub fn visit_design(&mut self, design: &Design, path: &str) {
        self.record("design", path);
        for item in &design.items {
            match item {
                DesignItem::Directive(directive) => {
                    self.record("design-item.directive", path);
                    self.visit_directive(directive, path);
                }
                DesignItem::Module(module) => {
                    self.record("design-item.module", path);
                    self.visit_module(module, path);
                }
            }
        }
    }

    pub fn count(&self, name: &str) -> usize {
        self.records.get(name).map_or(0, |record| record.count)
    }

    pub fn render(&self) -> String {
        let mut output = String::from("ast coverage:\n");
        for (name, record) in &self.records {
            let examples = record.files.iter().take(3).cloned().collect::<Vec<_>>();
            output.push_str(&format!(
                "  {name} | count={} | files={} | examples={}\n",
                record.count,
                record.files.len(),
                examples.join(",")
            ));
        }
        output
    }

    fn record(&mut self, name: impl Into<String>, path: &str) {
        let record = self.records.entry(name.into()).or_default();
        record.count += 1;
        record.files.insert(path.to_string());
    }

    fn visit_span(&mut self, kind: &str, span: &Span, path: &str) {
        assert_eq!(span.path, Path::new(path), "wrong logical path for {kind}");
        assert!(span.line > 0, "zero line for {kind} in {path}");
        assert!(span.column > 0, "zero column for {kind} in {path}");
        assert!(!span.path.is_absolute(), "absolute AST path for {kind}");
        assert!(
            span.path
                .components()
                .all(|component| !matches!(component, Component::ParentDir)),
            "parent traversal in AST path for {kind}"
        );
        self.record(format!("span.{kind}"), path);
    }

    fn visit_directive(&mut self, directive: &Directive, path: &str) {
        self.visit_span("directive", &directive.span, path);
        self.record(format!("directive.name.{}", directive.name), path);
        self.record(
            format!("directive.arguments.{}", directive.arguments.join("-")),
            path,
        );
    }

    fn visit_module(&mut self, module: &Module, path: &str) {
        self.visit_span("module", &module.span, path);
        self.record("module", path);
        for parameter in &module.parameters {
            self.visit_parameter(parameter, path);
        }
        for port in &module.ports {
            self.visit_port(port, path);
        }
        for item in &module.items {
            self.visit_item(item, path);
        }
    }

    fn visit_parameter(&mut self, parameter: &ParamDecl, path: &str) {
        self.visit_span("parameter", &parameter.span, path);
        let kind = match parameter.kind {
            ParamKind::Parameter => "parameter",
            ParamKind::Localparam => "localparam",
            ParamKind::Specparam => "specparam",
        };
        self.record(format!("parameter-kind.{kind}"), path);
        if let Some(ty) = &parameter.ty {
            self.record(format!("parameter-type.{ty}"), path);
        }
        self.visit_expr(&parameter.value, path);
    }

    fn visit_port(&mut self, port: &PortDecl, path: &str) {
        self.visit_span("port", &port.span, path);
        let direction = match port.direction {
            Direction::Input => "input",
            Direction::Output => "output",
            Direction::Inout => "inout",
        };
        self.record(format!("direction.{direction}"), path);
        self.record(format!("port.names.arity-{}", port.names.len()), path);
        for modifier in &port.modifiers {
            self.record(format!("port.modifier.{modifier}"), path);
        }
    }

    fn visit_item(&mut self, item: &Item, path: &str) {
        self.visit_span("item", &item.span, path);
        self.record("item.total", path);
        match &item.kind {
            ItemKind::Import(import) => {
                self.record("item-kind.import", path);
                self.visit_span("import", &import.span, path);
                self.record(
                    format!(
                        "import.{}{}",
                        import.path.join("::"),
                        if import.wildcard { "::*" } else { "" }
                    ),
                    path,
                );
            }
            ItemKind::Decl(decl) => {
                self.record("item-kind.decl", path);
                self.visit_decl(decl, path);
            }
            ItemKind::Initial(assign) => {
                self.record("item-kind.initial", path);
                self.visit_assign_stmt(assign, path);
            }
            ItemKind::ProcAssign(assign) => {
                self.record("item-kind.proc-assign", path);
                self.visit_assign_stmt(assign, path);
            }
            ItemKind::AlwaysLatch(always) => {
                self.record("item-kind.always-latch", path);
                self.visit_span("always-latch", &always.span, path);
                if let Some(condition) = &always.condition {
                    self.visit_expr(condition, path);
                }
                self.visit_item(&always.body, path);
            }
            ItemKind::Always(always) => {
                self.record("item-kind.always", path);
                self.visit_span("always", &always.span, path);
                let kind = match always.kind {
                    AlwaysKind::Plain => "plain",
                    AlwaysKind::Comb => "comb",
                    AlwaysKind::Ff => "ff",
                };
                self.record(format!("always-kind.{kind}"), path);
                if let Some(sensitivity) = &always.sensitivity {
                    self.visit_sensitivity(sensitivity, path);
                } else {
                    self.record("sensitivity-kind.missing", path);
                }
                self.visit_item(&always.body, path);
            }
            ItemKind::Assign(assign) => {
                self.record("item-kind.assign", path);
                self.visit_span("assign-decl", &assign.span, path);
                self.visit_assign_op(assign.op, path);
                if let Some(strength) = &assign.strength {
                    self.visit_strength(strength, path);
                }
                if let Some(delay) = &assign.delay {
                    self.visit_delay(delay, path);
                }
                self.visit_expr(&assign.target, path);
                self.visit_expr(&assign.value, path);
            }
            ItemKind::Primitive(primitive) => {
                self.record("item-kind.primitive", path);
                self.visit_span("primitive", &primitive.span, path);
                self.record(format!("primitive.name.{}", primitive.name), path);
                if let Some(strength) = &primitive.strength {
                    self.visit_strength(strength, path);
                }
                if let Some(delay) = &primitive.delay {
                    self.visit_delay(delay, path);
                }
                self.record(
                    format!("primitive.arguments.arity-{}", primitive.args.len()),
                    path,
                );
                for (index, argument) in primitive.args.iter().enumerate() {
                    if let Some(argument) = argument {
                        self.visit_expr(argument, path);
                    } else {
                        self.record(format!("primitive.arguments.omitted-{index}"), path);
                    }
                }
            }
            ItemKind::Instantiation(instance) => {
                self.record("item-kind.instantiation", path);
                self.visit_span("instantiation", &instance.span, path);
                self.record(format!("instantiation.module.{}", instance.module), path);
                for parameter in &instance.parameters {
                    self.visit_param_override(parameter, path);
                }
                for connection in &instance.connections {
                    self.visit_connection(connection, path);
                }
            }
            ItemKind::Specify(specify) => {
                self.record("item-kind.specify", path);
                self.visit_span("specify", &specify.span, path);
                for item in &specify.items {
                    match item {
                        SpecifyItem::Specparam(parameter) => {
                            self.record("specify-item.specparam", path);
                            self.visit_parameter(parameter, path);
                        }
                        SpecifyItem::Path(spec_path) => {
                            self.record("specify-item.path", path);
                            self.visit_span("specify-path", &spec_path.span, path);
                            for control in &spec_path.controls {
                                self.visit_expr(control, path);
                            }
                            self.visit_expr(&spec_path.target, path);
                            self.record(
                                format!("specify-path.delays.arity-{}", spec_path.delays.len()),
                                path,
                            );
                            for (index, delay) in spec_path.delays.iter().enumerate() {
                                if let Some(delay) = delay {
                                    self.visit_expr(delay, path);
                                } else {
                                    self.record(
                                        format!("specify-path.delay.omitted-{index}"),
                                        path,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            ItemKind::Generate(block) => {
                self.record("item-kind.generate", path);
                self.visit_block(block, path);
            }
            ItemKind::Block(block) => {
                self.record("item-kind.block", path);
                self.visit_block(block, path);
            }
            ItemKind::If(if_stmt) => {
                self.record("item-kind.if", path);
                self.visit_span("if", &if_stmt.span, path);
                self.visit_expr(&if_stmt.condition, path);
                self.visit_item(&if_stmt.then_branch, path);
                if let Some(else_branch) = &if_stmt.else_branch {
                    self.record("if.else-present", path);
                    self.visit_item(else_branch, path);
                } else {
                    self.record("if.else-missing", path);
                }
            }
            ItemKind::Empty => {
                self.record("item-kind.empty", path);
            }
        }
    }

    fn visit_decl(&mut self, decl: &Decl, path: &str) {
        self.visit_span("decl", &decl.span, path);
        let kind = match decl.kind {
            DeclKind::Logic => "logic",
            DeclKind::Tri => "tri",
            DeclKind::Wire => "wire",
            DeclKind::Parameter => "parameter",
            DeclKind::Localparam => "localparam",
            DeclKind::Specparam => "specparam",
        };
        self.record(format!("decl-kind.{kind}"), path);
        if let Some(ty) = &decl.ty {
            self.record(format!("decl-type.{ty}"), path);
        }
        self.record(format!("decl.names.arity-{}", decl.names.len()), path);
        if let Some(value) = &decl.value {
            self.visit_expr(value, path);
        }
    }

    fn visit_assign_stmt(&mut self, assign: &AssignStmt, path: &str) {
        self.visit_span("assign-stmt", &assign.span, path);
        self.visit_assign_op(assign.op, path);
        self.visit_expr(&assign.target, path);
        self.visit_expr(&assign.value, path);
    }

    fn visit_assign_op(&mut self, op: AssignOp, path: &str) {
        let op = match op {
            AssignOp::Blocking => "blocking",
            AssignOp::NonBlocking => "nonblocking",
        };
        self.record(format!("assign-op.{op}"), path);
    }

    fn visit_sensitivity(&mut self, sensitivity: &Sensitivity, path: &str) {
        self.visit_span("sensitivity", &sensitivity.span, path);
        match &sensitivity.kind {
            SensitivityKind::Any => self.record("sensitivity-kind.any", path),
            SensitivityKind::List(events) => {
                self.record("sensitivity-kind.list", path);
                for event in events {
                    self.visit_span("event-control", &event.span, path);
                    if let Some(edge) = &event.edge {
                        self.record(format!("event-control.edge.{edge}"), path);
                    } else {
                        self.record("event-control.edge.missing", path);
                    }
                    if let Some(expr) = &event.expr {
                        self.visit_expr(expr, path);
                    } else {
                        self.record("event-control.expr.missing", path);
                    }
                }
            }
        }
    }

    fn visit_param_override(&mut self, parameter: &ParamOverride, path: &str) {
        self.visit_span("parameter-override", &parameter.span, path);
        match &parameter.kind {
            ParamOverrideKind::Named { value, .. } => {
                self.record("parameter-override-kind.named", path);
                self.visit_expr(value, path);
            }
            ParamOverrideKind::Positional(Some(value)) => {
                self.record("parameter-override-kind.positional", path);
                self.visit_expr(value, path);
            }
            ParamOverrideKind::Positional(None) => {
                self.record("parameter-override-kind.positional-omitted", path);
            }
        }
    }

    fn visit_connection(&mut self, connection: &Connection, path: &str) {
        self.visit_span("connection", &connection.span, path);
        match &connection.kind {
            ConnectionKind::Named { value, .. } => {
                self.record("connection-kind.named", path);
                self.visit_expr(value, path);
            }
            ConnectionKind::Positional(value) => {
                self.record("connection-kind.positional", path);
                self.visit_expr(value, path);
            }
        }
    }

    fn visit_strength(&mut self, strength: &Strength, path: &str) {
        self.visit_span("strength", &strength.span, path);
        self.record(
            format!("strength.shape.arity-{}", strength.values.len()),
            path,
        );
        self.record(
            format!("strength.values.{}", strength.values.join("-")),
            path,
        );
    }

    fn visit_delay(&mut self, delay: &Delay, path: &str) {
        self.visit_span("delay", &delay.span, path);
        self.record(format!("delay.shape.arity-{}", delay.values.len()), path);
        for (index, value) in delay.values.iter().enumerate() {
            if let Some(value) = value {
                self.visit_expr(value, path);
            } else {
                self.record(format!("delay.omitted-{index}"), path);
            }
        }
    }

    fn visit_block(&mut self, block: &Block, path: &str) {
        self.visit_span("block", &block.span, path);
        for item in &block.items {
            self.visit_item(item, path);
        }
    }

    fn visit_expr(&mut self, expr: &Expr, path: &str) {
        self.visit_span("expr", &expr.span, path);
        match &expr.kind {
            ExprKind::Path(segments) => {
                self.record("expr-kind.path", path);
                self.record(format!("expr.path.segments-{}", segments.len()), path);
            }
            ExprKind::Integer(_) => self.record("expr-kind.integer", path),
            ExprKind::Real(_) => self.record("expr-kind.real", path),
            ExprKind::Constant(kind) => {
                self.record("expr-kind.constant", path);
                let kind = match kind {
                    ConstKind::Zero => "zero",
                    ConstKind::One => "one",
                    ConstKind::Z => "z",
                    ConstKind::X => "x",
                };
                self.record(format!("const-kind.{kind}"), path);
            }
            ExprKind::Group(inner) => {
                self.record("expr-kind.group", path);
                self.visit_expr(inner, path);
            }
            ExprKind::Unary { op, expr } => {
                self.record("expr-kind.unary", path);
                let op = match op {
                    UnaryOp::Not => "not",
                    UnaryOp::BitNot => "bit-not",
                    UnaryOp::Plus => "plus",
                    UnaryOp::Minus => "minus",
                };
                self.record(format!("unary-op.{op}"), path);
                self.visit_expr(expr, path);
            }
            ExprKind::Binary { op, left, right } => {
                self.record("expr-kind.binary", path);
                let op = match op {
                    BinaryOp::Mul => "mul",
                    BinaryOp::Div => "div",
                    BinaryOp::Add => "add",
                    BinaryOp::Sub => "sub",
                    BinaryOp::BitAnd => "bit-and",
                    BinaryOp::BitOr => "bit-or",
                    BinaryOp::BitXor => "bit-xor",
                    BinaryOp::BitNand => "bit-nand",
                    BinaryOp::BitNor => "bit-nor",
                    BinaryOp::BitXnor => "bit-xnor",
                    BinaryOp::LogicalAnd => "logical-and",
                    BinaryOp::LogicalOr => "logical-or",
                    BinaryOp::Eq => "eq",
                    BinaryOp::CaseEq => "case-eq",
                    BinaryOp::Neq => "neq",
                    BinaryOp::CaseNeq => "case-neq",
                    BinaryOp::Less => "less",
                    BinaryOp::Greater => "greater",
                };
                self.record(format!("binary-op.{op}"), path);
                self.visit_expr(left, path);
                self.visit_expr(right, path);
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                self.record("expr-kind.ternary", path);
                self.visit_expr(condition, path);
                self.visit_expr(then_expr, path);
                self.visit_expr(else_expr, path);
            }
            ExprKind::Call { callee, args } => {
                self.record("expr-kind.call", path);
                self.record(format!("call.arguments.arity-{}", args.len()), path);
                self.visit_expr(callee, path);
                for (index, argument) in args.iter().enumerate() {
                    if let Some(argument) = argument {
                        self.visit_expr(argument, path);
                    } else {
                        self.record(format!("call.arguments.omitted-{index}"), path);
                    }
                }
            }
        }
    }
}

pub fn assert_or_update_fixture(path: &Path, actual: &str) {
    if std::env::var_os("UPDATE_AST_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().expect("fixture path has a parent")).unwrap();
        fs::write(path, actual).unwrap();
    }
    let expected = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(actual, expected, "fixture {}", path.display());
}
