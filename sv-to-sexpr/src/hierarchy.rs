//! Typed, deterministic flattening of ordinary module instances.
//!
//! Catalog-aware analysis remains the user-visible record of parameter and
//! port binding. This module consumes those same typed binding rules and
//! rewrites a generate-selected AST for the flat lowering pipeline.

use crate::analyze::{
    InstantiationResolution, ModuleCatalog, ParameterBindingSource, ResolvedInstantiation,
    is_special_instance, resolve_ast_instantiation,
};
use crate::ast::*;
use crate::diagnostic::{Diagnostic, Span};
use crate::elaborate::{GenerateMode, elaborate_design};
use std::collections::{BTreeMap, BTreeSet};

pub type HierarchyResult<T> = Result<T, Diagnostic>;

/// Selects generate branches and recursively replaces ordinary instances by
/// qualified child items. Special instances such as `keeper` remain typed AST
/// instantiations for their dedicated lowering milestone.
pub fn flatten_design_with_catalog_and_generate_mode(
    design: &Design,
    catalog: &ModuleCatalog,
    mode: GenerateMode,
) -> HierarchyResult<Design> {
    let elaborated = elaborate_design(design, mode)?;
    let mut items = Vec::with_capacity(elaborated.items.len());
    for item in &elaborated.items {
        items.push(match item {
            DesignItem::Directive(directive) => DesignItem::Directive(directive.clone()),
            DesignItem::Module(module) => {
                DesignItem::Module(FlattenContext::new(catalog, mode, module).flatten_root(module)?)
            }
        });
    }
    Ok(Design { items })
}

struct FlattenContext<'a> {
    catalog: &'a ModuleCatalog,
    mode: GenerateMode,
    reserved: BTreeMap<String, Span>,
    signals: BTreeMap<String, Span>,
}

#[derive(Debug, Clone, Default)]
struct Environment {
    substitutions: BTreeMap<String, Expr>,
    prefix: Option<String>,
}

impl Environment {
    fn qualified(&self, name: &str) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}__{name}"),
            None => name.to_string(),
        }
    }
}

impl<'a> FlattenContext<'a> {
    fn new(catalog: &'a ModuleCatalog, mode: GenerateMode, root: &Module) -> Self {
        let mut reserved = BTreeMap::new();
        collect_module_visible_names(root, &mut reserved);
        let mut signals = BTreeMap::new();
        collect_module_signal_names(root, &mut signals);
        Self {
            catalog,
            mode,
            reserved,
            signals,
        }
    }

    fn flatten_root(&mut self, module: &Module) -> HierarchyResult<Module> {
        let mut stack = vec![module.name.clone()];
        let items = self.transform_items(
            &module.items,
            &Environment::default(),
            &module.name,
            &mut stack,
        )?;
        Ok(Module {
            span: module.span.clone(),
            name: module.name.clone(),
            parameters: module.parameters.clone(),
            ports: module.ports.clone(),
            items,
        })
    }

    fn transform_items(
        &mut self,
        items: &[Item],
        environment: &Environment,
        current_module: &str,
        stack: &mut Vec<String>,
    ) -> HierarchyResult<Vec<Item>> {
        let mut transformed = Vec::new();
        for item in items {
            transformed.extend(self.transform_item(item, environment, current_module, stack)?);
        }
        Ok(transformed)
    }

    fn transform_item(
        &mut self,
        item: &Item,
        environment: &Environment,
        current_module: &str,
        stack: &mut Vec<String>,
    ) -> HierarchyResult<Vec<Item>> {
        let kind = match &item.kind {
            ItemKind::Import(import) => ItemKind::Import(import.clone()),
            ItemKind::Decl(declaration) => ItemKind::Decl(Decl {
                span: declaration.span.clone(),
                kind: declaration.kind,
                ty: declaration.ty.clone(),
                names: declaration
                    .names
                    .iter()
                    .map(|name| transformed_name(name, environment))
                    .collect(),
                value: declaration
                    .value
                    .as_ref()
                    .map(|value| transform_expr(value, environment)),
            }),
            ItemKind::Initial(statement) => {
                ItemKind::Initial(transform_assignment(statement, environment))
            }
            ItemKind::ProcAssign(statement) => {
                ItemKind::ProcAssign(transform_assignment(statement, environment))
            }
            ItemKind::AlwaysLatch(always) => ItemKind::AlwaysLatch(AlwaysLatch {
                span: always.span.clone(),
                condition: always
                    .condition
                    .as_ref()
                    .map(|condition| transform_expr(condition, environment)),
                body: Box::new(self.transform_body(
                    &always.body,
                    environment,
                    current_module,
                    stack,
                )?),
            }),
            ItemKind::Always(always) => ItemKind::Always(AlwaysBlock {
                span: always.span.clone(),
                kind: always.kind,
                sensitivity: always
                    .sensitivity
                    .as_ref()
                    .map(|sensitivity| transform_sensitivity(sensitivity, environment)),
                body: Box::new(self.transform_body(
                    &always.body,
                    environment,
                    current_module,
                    stack,
                )?),
            }),
            ItemKind::Assign(assignment) => ItemKind::Assign(AssignDecl {
                span: assignment.span.clone(),
                strength: assignment.strength.clone(),
                delay: assignment
                    .delay
                    .as_ref()
                    .map(|delay| transform_delay(delay, environment)),
                target: transform_expr(&assignment.target, environment),
                value: transform_expr(&assignment.value, environment),
                op: assignment.op,
            }),
            ItemKind::Primitive(call) => ItemKind::Primitive(PrimitiveCall {
                span: call.span.clone(),
                name: call.name.clone(),
                strength: call.strength.clone(),
                delay: call
                    .delay
                    .as_ref()
                    .map(|delay| transform_delay(delay, environment)),
                args: call
                    .args
                    .iter()
                    .map(|argument| {
                        argument
                            .as_ref()
                            .map(|argument| transform_expr(argument, environment))
                    })
                    .collect(),
            }),
            ItemKind::Instantiation(instantiation) => {
                return self.transform_instantiation(
                    instantiation,
                    environment,
                    current_module,
                    stack,
                );
            }
            ItemKind::Specify(specify) => {
                ItemKind::Specify(transform_specify(specify, environment))
            }
            ItemKind::Generate(block) => ItemKind::Generate(Block {
                span: block.span.clone(),
                items: self.transform_items(&block.items, environment, current_module, stack)?,
            }),
            ItemKind::Block(block) => ItemKind::Block(Block {
                span: block.span.clone(),
                items: self.transform_items(&block.items, environment, current_module, stack)?,
            }),
            ItemKind::If(statement) => ItemKind::If(IfStmt {
                span: statement.span.clone(),
                condition: transform_expr(&statement.condition, environment),
                then_branch: Box::new(self.transform_body(
                    &statement.then_branch,
                    environment,
                    current_module,
                    stack,
                )?),
                else_branch: statement
                    .else_branch
                    .as_ref()
                    .map(|branch| {
                        self.transform_body(branch, environment, current_module, stack)
                            .map(Box::new)
                    })
                    .transpose()?,
            }),
            ItemKind::Empty => ItemKind::Empty,
        };
        Ok(vec![Item {
            span: item.span.clone(),
            kind,
        }])
    }

    fn transform_body(
        &mut self,
        body: &Item,
        environment: &Environment,
        current_module: &str,
        stack: &mut Vec<String>,
    ) -> HierarchyResult<Item> {
        let mut transformed = self.transform_item(body, environment, current_module, stack)?;
        if transformed.len() == 1 {
            return Ok(transformed.remove(0));
        }
        Ok(Item {
            span: body.span.clone(),
            kind: ItemKind::Block(Block {
                span: body.span.clone(),
                items: transformed,
            }),
        })
    }

    fn transform_instantiation(
        &mut self,
        instantiation: &Instantiation,
        environment: &Environment,
        current_module: &str,
        stack: &mut Vec<String>,
    ) -> HierarchyResult<Vec<Item>> {
        let transformed = Instantiation {
            span: instantiation.span.clone(),
            module: instantiation.module.clone(),
            parameters: instantiation
                .parameters
                .iter()
                .map(|parameter| ParamOverride {
                    span: parameter.span.clone(),
                    kind: match &parameter.kind {
                        ParamOverrideKind::Named { name, value } => ParamOverrideKind::Named {
                            name: name.clone(),
                            value: transform_expr(value, environment),
                        },
                        ParamOverrideKind::Positional(value) => ParamOverrideKind::Positional(
                            value
                                .as_ref()
                                .map(|value| transform_expr(value, environment)),
                        ),
                    },
                })
                .collect(),
            instance: environment.qualified(&instantiation.instance),
            connections: instantiation
                .connections
                .iter()
                .map(|connection| Connection {
                    span: connection.span.clone(),
                    kind: match &connection.kind {
                        ConnectionKind::Named { name, value } => ConnectionKind::Named {
                            name: name.clone(),
                            value: transform_expr(value, environment),
                        },
                        ConnectionKind::Positional(value) => {
                            ConnectionKind::Positional(transform_expr(value, environment))
                        }
                    },
                })
                .collect(),
        };

        if is_special_instance(&transformed.module) {
            return Ok(vec![Item {
                span: instantiation.span.clone(),
                kind: ItemKind::Instantiation(transformed),
            }]);
        }
        if stack.iter().any(|module| module == &transformed.module) {
            return Err(Diagnostic::new(
                instantiation.span.clone(),
                format!(
                    "recursive module reference from `{current_module}` through `{}`",
                    transformed.module
                ),
            ));
        }

        let resolution = resolve_ast_instantiation(
            current_module,
            &transformed,
            &self.signals,
            self.catalog,
            Some(self.mode),
        )?;
        let InstantiationResolution::Resolved(resolved) = resolution else {
            unreachable!("ordinary instance must resolve through a catalog definition")
        };
        let child = self.elaborated_definition(&transformed.module, &transformed.span)?;
        let child_environment = self.child_environment(&transformed, &child, &resolved)?;

        stack.push(transformed.module.clone());
        let result =
            self.transform_items(&child.items, &child_environment, &transformed.module, stack);
        stack.pop();
        result
    }

    fn elaborated_definition(&self, name: &str, span: &Span) -> HierarchyResult<Module> {
        let definition = self.catalog.definition(name).ok_or_else(|| {
            Diagnostic::new(
                span.clone(),
                format!("unknown instantiated module `{name}`"),
            )
        })?;
        let design = Design {
            items: vec![DesignItem::Module(definition.clone())],
        };
        let elaborated = elaborate_design(&design, self.mode)?;
        Ok(elaborated
            .first_module()
            .expect("single catalog definition remains a module")
            .clone())
    }

    fn child_environment(
        &mut self,
        instantiation: &Instantiation,
        child: &Module,
        resolved: &ResolvedInstantiation,
    ) -> HierarchyResult<Environment> {
        let prefix = instantiation.instance.clone();
        let mut substitutions = resolve_parameter_environment(
            child,
            resolved,
            &Environment {
                substitutions: BTreeMap::new(),
                prefix: None,
            },
        )?;

        for connection in &resolved.connections {
            if substitutions
                .insert(connection.port.clone(), connection.expression.clone())
                .is_some()
            {
                return Err(Diagnostic::new(
                    connection.span.clone(),
                    format!("duplicate child symbol `{}`", connection.port),
                ));
            }
        }

        let mut locals = BTreeMap::new();
        collect_item_visible_names(&child.items, &mut locals);
        let mut local_signals = BTreeMap::new();
        collect_item_signal_names(&child.items, &mut local_signals);
        for (name, span) in locals {
            if substitutions.contains_key(&name) {
                return Err(Diagnostic::new(
                    span,
                    format!("child symbol `{name}` conflicts with a parameter or port"),
                ));
            }
            let qualified = format!("{prefix}__{name}");
            if let Some(previous) = self.reserved.get(&qualified) {
                return Err(Diagnostic::new(
                    instantiation.span.clone(),
                    format!(
                        "qualified hierarchy name `{qualified}` collides with symbol declared at {}:{}:{}",
                        previous.path.display(),
                        previous.line,
                        previous.column
                    ),
                ));
            }
            self.reserved.insert(qualified.clone(), span.clone());
            if local_signals.contains_key(&name) {
                self.signals.insert(qualified.clone(), span.clone());
            }
            substitutions.insert(
                name,
                Expr {
                    span,
                    kind: ExprKind::Path(vec![qualified]),
                },
            );
        }

        Ok(Environment {
            substitutions,
            prefix: Some(prefix),
        })
    }
}

fn resolve_parameter_environment(
    child: &Module,
    resolved: &ResolvedInstantiation,
    outer: &Environment,
) -> HierarchyResult<BTreeMap<String, Expr>> {
    let bindings = resolved
        .parameter_bindings
        .iter()
        .map(|binding| (binding.parameter.clone(), binding.clone()))
        .collect::<BTreeMap<_, _>>();
    let parameter_names = child
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect::<BTreeSet<_>>();
    let mut values = BTreeMap::new();
    let mut visiting = Vec::new();
    for parameter in &child.parameters {
        resolve_parameter(
            &parameter.name,
            &bindings,
            &parameter_names,
            outer,
            &mut values,
            &mut visiting,
        )?;
    }
    Ok(values)
}

fn resolve_parameter(
    name: &str,
    bindings: &BTreeMap<String, crate::analyze::ResolvedParameterBinding>,
    parameter_names: &BTreeSet<String>,
    outer: &Environment,
    values: &mut BTreeMap<String, Expr>,
    visiting: &mut Vec<String>,
) -> HierarchyResult<Expr> {
    if let Some(value) = values.get(name) {
        return Ok(value.clone());
    }
    let binding = bindings
        .get(name)
        .expect("analysis resolves every interface parameter");
    if let Some(index) = visiting.iter().position(|active| active == name) {
        let mut cycle = visiting[index..].to_vec();
        cycle.push(name.to_string());
        return Err(Diagnostic::new(
            binding.span.clone(),
            format!("cyclic parameter dependency: {}", cycle.join(" -> ")),
        ));
    }
    visiting.push(name.to_string());
    let value = match binding.source {
        ParameterBindingSource::Named | ParameterBindingSource::Positional { .. } => {
            transform_expr(&binding.expression, outer)
        }
        ParameterBindingSource::Default | ParameterBindingSource::OmittedPositional { .. } => {
            transform_default_expr(
                &binding.expression,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?
        }
    };
    visiting.pop();
    values.insert(name.to_string(), value.clone());
    Ok(value)
}

fn transform_default_expr(
    expression: &Expr,
    bindings: &BTreeMap<String, crate::analyze::ResolvedParameterBinding>,
    parameter_names: &BTreeSet<String>,
    outer: &Environment,
    values: &mut BTreeMap<String, Expr>,
    visiting: &mut Vec<String>,
) -> HierarchyResult<Expr> {
    if let ExprKind::Path(path) = &expression.kind
        && path.len() == 1
        && parameter_names.contains(&path[0])
    {
        return resolve_parameter(&path[0], bindings, parameter_names, outer, values, visiting);
    }
    let kind = match &expression.kind {
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {
            expression.kind.clone()
        }
        ExprKind::Group(inner) => ExprKind::Group(Box::new(transform_default_expr(
            inner,
            bindings,
            parameter_names,
            outer,
            values,
            visiting,
        )?)),
        ExprKind::Unary { op, expr } => ExprKind::Unary {
            op: *op,
            expr: Box::new(transform_default_expr(
                expr,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
        },
        ExprKind::Binary { op, left, right } => ExprKind::Binary {
            op: *op,
            left: Box::new(transform_default_expr(
                left,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
            right: Box::new(transform_default_expr(
                right,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
        },
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => ExprKind::Ternary {
            condition: Box::new(transform_default_expr(
                condition,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
            then_expr: Box::new(transform_default_expr(
                then_expr,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
            else_expr: Box::new(transform_default_expr(
                else_expr,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
        },
        ExprKind::Call { callee, args } => ExprKind::Call {
            callee: Box::new(transform_default_expr(
                callee,
                bindings,
                parameter_names,
                outer,
                values,
                visiting,
            )?),
            args: args
                .iter()
                .map(|argument| {
                    argument
                        .as_ref()
                        .map(|argument| {
                            transform_default_expr(
                                argument,
                                bindings,
                                parameter_names,
                                outer,
                                values,
                                visiting,
                            )
                        })
                        .transpose()
                })
                .collect::<HierarchyResult<Vec<_>>>()?,
        },
    };
    Ok(Expr {
        span: expression.span.clone(),
        kind,
    })
}

fn transform_expr(expression: &Expr, environment: &Environment) -> Expr {
    if let ExprKind::Path(path) = &expression.kind
        && path.len() == 1
        && let Some(value) = environment.substitutions.get(&path[0])
    {
        return value.clone();
    }
    let kind = match &expression.kind {
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {
            expression.kind.clone()
        }
        ExprKind::Group(inner) => ExprKind::Group(Box::new(transform_expr(inner, environment))),
        ExprKind::Unary { op, expr } => ExprKind::Unary {
            op: *op,
            expr: Box::new(transform_expr(expr, environment)),
        },
        ExprKind::Binary { op, left, right } => ExprKind::Binary {
            op: *op,
            left: Box::new(transform_expr(left, environment)),
            right: Box::new(transform_expr(right, environment)),
        },
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => ExprKind::Ternary {
            condition: Box::new(transform_expr(condition, environment)),
            then_expr: Box::new(transform_expr(then_expr, environment)),
            else_expr: Box::new(transform_expr(else_expr, environment)),
        },
        ExprKind::Call { callee, args } => ExprKind::Call {
            callee: Box::new(transform_expr(callee, environment)),
            args: args
                .iter()
                .map(|argument| {
                    argument
                        .as_ref()
                        .map(|argument| transform_expr(argument, environment))
                })
                .collect(),
        },
    };
    Expr {
        span: expression.span.clone(),
        kind,
    }
}

fn transform_assignment(statement: &AssignStmt, environment: &Environment) -> AssignStmt {
    AssignStmt {
        span: statement.span.clone(),
        target: transform_expr(&statement.target, environment),
        value: transform_expr(&statement.value, environment),
        op: statement.op,
    }
}

fn transform_delay(delay: &Delay, environment: &Environment) -> Delay {
    Delay {
        span: delay.span.clone(),
        values: delay
            .values
            .iter()
            .map(|value| {
                value
                    .as_ref()
                    .map(|value| transform_expr(value, environment))
            })
            .collect(),
    }
}

fn transform_sensitivity(sensitivity: &Sensitivity, environment: &Environment) -> Sensitivity {
    Sensitivity {
        span: sensitivity.span.clone(),
        kind: match &sensitivity.kind {
            SensitivityKind::Any => SensitivityKind::Any,
            SensitivityKind::List(events) => SensitivityKind::List(
                events
                    .iter()
                    .map(|event| EventControl {
                        span: event.span.clone(),
                        edge: event.edge.clone(),
                        expr: event
                            .expr
                            .as_ref()
                            .map(|expression| transform_expr(expression, environment)),
                    })
                    .collect(),
            ),
        },
    }
}

fn transform_specify(specify: &SpecifyBlock, environment: &Environment) -> SpecifyBlock {
    SpecifyBlock {
        span: specify.span.clone(),
        items: specify
            .items
            .iter()
            .map(|item| match item {
                SpecifyItem::Specparam(parameter) => SpecifyItem::Specparam(ParamDecl {
                    span: parameter.span.clone(),
                    kind: parameter.kind,
                    ty: parameter.ty.clone(),
                    name: transformed_name(&parameter.name, environment),
                    value: transform_expr(&parameter.value, environment),
                }),
                SpecifyItem::Path(path) => SpecifyItem::Path(SpecPath {
                    span: path.span.clone(),
                    controls: path
                        .controls
                        .iter()
                        .map(|control| transform_expr(control, environment))
                        .collect(),
                    target: transform_expr(&path.target, environment),
                    delays: path
                        .delays
                        .iter()
                        .map(|delay| {
                            delay
                                .as_ref()
                                .map(|delay| transform_expr(delay, environment))
                        })
                        .collect(),
                }),
            })
            .collect(),
    }
}

fn transformed_name(name: &str, environment: &Environment) -> String {
    environment
        .substitutions
        .get(name)
        .and_then(|expression| match &expression.kind {
            ExprKind::Path(path) if path.len() == 1 => Some(path[0].clone()),
            _ => None,
        })
        .unwrap_or_else(|| environment.qualified(name))
}

fn collect_module_visible_names(module: &Module, visible: &mut BTreeMap<String, Span>) {
    for parameter in &module.parameters {
        visible.insert(parameter.name.clone(), parameter.span.clone());
    }
    for port in &module.ports {
        for name in &port.names {
            visible.insert(name.clone(), port.span.clone());
        }
    }
    collect_item_visible_names(&module.items, visible);
}

fn collect_module_signal_names(module: &Module, signals: &mut BTreeMap<String, Span>) {
    for port in &module.ports {
        for name in &port.names {
            signals.insert(name.clone(), port.span.clone());
        }
    }
    collect_item_signal_names(&module.items, signals);
}

fn collect_item_signal_names(items: &[Item], signals: &mut BTreeMap<String, Span>) {
    for item in items {
        match &item.kind {
            ItemKind::Decl(declaration)
                if matches!(
                    declaration.kind,
                    DeclKind::Logic | DeclKind::Tri | DeclKind::Wire
                ) =>
            {
                for name in &declaration.names {
                    signals.insert(name.clone(), declaration.span.clone());
                }
            }
            ItemKind::AlwaysLatch(always) => {
                collect_item_signal_names(std::slice::from_ref(always.body.as_ref()), signals)
            }
            ItemKind::Always(always) => {
                collect_item_signal_names(std::slice::from_ref(always.body.as_ref()), signals)
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                collect_item_signal_names(&block.items, signals);
            }
            ItemKind::If(statement) => {
                collect_item_signal_names(
                    std::slice::from_ref(statement.then_branch.as_ref()),
                    signals,
                );
                if let Some(branch) = &statement.else_branch {
                    collect_item_signal_names(std::slice::from_ref(branch.as_ref()), signals);
                }
            }
            ItemKind::Import(_)
            | ItemKind::Decl(_)
            | ItemKind::Initial(_)
            | ItemKind::ProcAssign(_)
            | ItemKind::Assign(_)
            | ItemKind::Primitive(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Specify(_)
            | ItemKind::Empty => {}
        }
    }
}

fn collect_item_visible_names(items: &[Item], visible: &mut BTreeMap<String, Span>) {
    for item in items {
        match &item.kind {
            ItemKind::Decl(declaration) => {
                for name in &declaration.names {
                    visible.insert(name.clone(), declaration.span.clone());
                }
            }
            ItemKind::Specify(specify) => {
                for specify_item in &specify.items {
                    if let SpecifyItem::Specparam(parameter) = specify_item {
                        visible.insert(parameter.name.clone(), parameter.span.clone());
                    }
                }
            }
            ItemKind::AlwaysLatch(always) => {
                collect_item_visible_names(std::slice::from_ref(always.body.as_ref()), visible)
            }
            ItemKind::Always(always) => {
                collect_item_visible_names(std::slice::from_ref(always.body.as_ref()), visible)
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                collect_item_visible_names(&block.items, visible);
            }
            ItemKind::If(statement) => {
                collect_item_visible_names(
                    std::slice::from_ref(statement.then_branch.as_ref()),
                    visible,
                );
                if let Some(branch) = &statement.else_branch {
                    collect_item_visible_names(std::slice::from_ref(branch.as_ref()), visible);
                }
            }
            ItemKind::Import(_)
            | ItemKind::Initial(_)
            | ItemKind::ProcAssign(_)
            | ItemKind::Assign(_)
            | ItemKind::Primitive(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Empty => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CellItem, Expr as IrExpr};
    use crate::lower::lower_design_with_catalog;
    use crate::parser::parse_file;
    use std::path::Path;

    fn parse(path: &str, source: &str) -> Design {
        parse_file(Path::new(path), source).unwrap()
    }

    fn assignments(lowered: &crate::ir::LoweredModule) -> Vec<(&str, &IrExpr, &IrExpr)> {
        lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some((
                    assignment.target.as_str(),
                    &assignment.expr,
                    &assignment.delay,
                )),
                CellItem::Blank | CellItem::Comment(_) => None,
            })
            .collect()
    }

    const PARAM_CHILD: &str = r#"module param_child #(
  parameter real P = 3,
  parameter real Q = P
) (input logic ci1, ci2, output logic co);
  logic local;
  localparam real T = Q;
  assign local = ci1 & ci2;
  assign #(T) co = local;
endmodule
"#;

    #[test]
    fn substitutes_named_positional_omitted_and_default_bindings_and_flattens_compound_inputs() {
        let child = parse("param_child.sv", PARAM_CHILD);
        let root = parse(
            "root.sv",
            r#"module root(input logic a, b, output logic y1, y2, y3);
  param_child #(.P(7)) u1(.co(y1), .ci2(b), .ci1(a | b));
  param_child #(8, ) u2(a, b, y2);
  param_child u3(.ci1(a), .ci2(b), .co(y3));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[root.clone(), child]).unwrap();
        let lowered = lower_design_with_catalog(&root, &catalog).unwrap();
        let assignments = assignments(&lowered);

        assert_eq!(
            assignments.iter().map(|item| item.0).collect::<Vec<_>>(),
            [
                "t0",
                "u1__local",
                "y1",
                "u2__local",
                "y2",
                "u3__local",
                "y3"
            ]
        );
        assert_eq!(
            assignments[0].1,
            &IrExpr::List(vec![
                IrExpr::Atom("or".into()),
                IrExpr::Atom("a".into()),
                IrExpr::Atom("b".into()),
            ])
        );
        assert_eq!(assignments[2].2, &IrExpr::Atom("7".into()));
        assert_eq!(assignments[4].2, &IrExpr::Atom("8".into()));
        assert_eq!(assignments[6].2, &IrExpr::Atom("3".into()));
        assert_eq!(lowered.timing_aliases["u1__T"], IrExpr::Atom("7".into()));
        assert_eq!(lowered.timing_aliases["u2__T"], IrExpr::Atom("8".into()));
        assert_eq!(lowered.timing_aliases["u3__T"], IrExpr::Atom("3".into()));
        assert!(!format!("{lowered:?}").contains("ci1"));
        assert!(!format!("{lowered:?}").contains("ci2"));
        assert!(!format!("{lowered:?}").contains("co"));
    }

    #[test]
    fn repeated_instances_get_qualified_locals_and_preserve_instance_then_child_order() {
        let child = parse("param_child.sv", PARAM_CHILD);
        let root = parse(
            "root.sv",
            r#"module root(input logic a, b, output logic y1, y2);
  param_child u1(a, b, y1);
  param_child u2(b, a, y2);
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[root.clone(), child]).unwrap();
        let first = lower_design_with_catalog(&root, &catalog).unwrap();
        let second = lower_design_with_catalog(&root, &catalog).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            assignments(&first)
                .iter()
                .map(|assignment| assignment.0)
                .collect::<Vec<_>>(),
            ["u1__local", "y1", "u2__local", "y2"]
        );
        assert_eq!(
            first
                .timing_aliases
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["u1__T", "u2__T"]
        );
    }

    #[test]
    fn substitutes_parameter_bindings_into_value_expressions_as_typed_ast() {
        let child = parse(
            "value_child.sv",
            r#"module value_child #(parameter real SELECT = 0) (input logic a, output logic y);
  assign y = SELECT ? a : ~a;
endmodule
"#,
        );
        let root = parse(
            "root.sv",
            r#"module root(input logic a, select, output logic y1, y2);
  value_child #(.SELECT(select)) u1(.a(a), .y(y1));
  value_child u2(.a(a), .y(y2));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[root.clone(), child]).unwrap();
        let lowered = lower_design_with_catalog(&root, &catalog).unwrap();
        let assignments = assignments(&lowered);
        assert_eq!(
            assignments.iter().map(|item| item.0).collect::<Vec<_>>(),
            ["t0", "y1", "t1", "y2"]
        );
        assert_eq!(
            assignments[1].1,
            &IrExpr::List(vec![
                IrExpr::Atom("mux".into()),
                IrExpr::Atom("select".into()),
                IrExpr::Atom("a".into()),
                IrExpr::Atom("t0".into()),
            ])
        );
        assert_eq!(
            assignments[3].1,
            &IrExpr::List(vec![
                IrExpr::Atom("mux".into()),
                IrExpr::Atom("0".into()),
                IrExpr::Atom("a".into()),
                IrExpr::Atom("t1".into()),
            ])
        );
    }

    #[test]
    fn recursively_flattens_nested_instances_with_stable_qualification() {
        let leaf = parse(
            "leaf.sv",
            r#"module leaf(input logic a, output logic y);
  logic local;
  assign local = ~a;
  assign y = local;
endmodule
"#,
        );
        let middle = parse(
            "middle.sv",
            r#"module middle(input logic a, output logic y);
  logic middle_local;
  leaf inner(.a(a), .y(middle_local));
  assign y = middle_local;
endmodule
"#,
        );
        let root = parse(
            "root.sv",
            r#"module root(input logic a, output logic y);
  middle outer(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[root.clone(), middle, leaf]).unwrap();
        let lowered = lower_design_with_catalog(&root, &catalog).unwrap();
        assert_eq!(
            assignments(&lowered)
                .iter()
                .map(|assignment| assignment.0)
                .collect::<Vec<_>>(),
            ["outer__inner__local", "outer__middle_local", "y"]
        );
    }

    #[test]
    fn leaves_special_keeper_instances_unflattened() {
        let root = parse(
            "keeper_root.sv",
            r#"module keeper_root(input logic a, output logic y);
  keeper hold(.o(y), .i(a));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&root)).unwrap();
        let flattened =
            flatten_design_with_catalog_and_generate_mode(&root, &catalog, GenerateMode::Delayful)
                .unwrap();
        let ItemKind::Instantiation(instance) = &flattened.first_module().unwrap().items[0].kind
        else {
            panic!("keeper must remain an instantiation")
        };
        assert_eq!(instance.module, "keeper");
        assert_eq!(instance.instance, "hold");
    }

    #[test]
    fn child_generate_selection_controls_recursive_dependency_checks() {
        let child = parse(
            "selectable_child.sv",
            r#"module selectable_child(input logic a, output logic y);
  generate
    if (nodelay) begin
      selectable_child recurse(.a(a), .y(y));
    end else begin
      assign y = a;
    end
  endgenerate
endmodule
"#,
        );
        let root = parse(
            "root.sv",
            r#"module root(input logic a, output logic y);
  selectable_child u(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[root.clone(), child]).unwrap();
        let delayful = lower_design_with_catalog(&root, &catalog).unwrap();
        assert_eq!(
            assignments(&delayful)
                .iter()
                .map(|assignment| assignment.0)
                .collect::<Vec<_>>(),
            ["y"]
        );

        let error = crate::lower::lower_design_with_catalog_and_generate_mode(
            &root,
            &catalog,
            GenerateMode::Nodelay,
        )
        .unwrap_err();
        assert_eq!(error.span, Span::new("selectable_child.sv", 4, 7));
        assert_eq!(
            error.message,
            "recursive module reference from `selectable_child` through `selectable_child`"
        );
    }

    #[test]
    fn rejects_unknown_modules_and_qualified_name_collisions_at_instance_spans() {
        let unknown = parse(
            "unknown.sv",
            r#"module unknown(input logic a, output logic y);
  absent u(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&unknown)).unwrap();
        let error = lower_design_with_catalog(&unknown, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("unknown.sv", 2, 3));
        assert_eq!(
            error.message,
            "unknown instantiated module `absent` for instance `u`"
        );

        let child = parse(
            "child.sv",
            r#"module child(input logic a, output logic y);
  logic local;
  assign local = a;
  assign y = local;
endmodule
"#,
        );
        let collision = parse(
            "collision.sv",
            r#"module collision(input logic a, output logic y);
  logic u__local;
  child u(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[collision.clone(), child]).unwrap();
        let error = lower_design_with_catalog(&collision, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("collision.sv", 3, 3));
        assert!(
            error.message.starts_with(
                "qualified hierarchy name `u__local` collides with symbol declared at"
            )
        );
    }

    #[test]
    fn rejects_direct_and_indirect_recursion_at_the_recursive_instance() {
        let direct = parse(
            "direct.sv",
            r#"module direct(input logic a, output logic y);
  direct again(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(std::slice::from_ref(&direct)).unwrap();
        let error = lower_design_with_catalog(&direct, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("direct.sv", 2, 3));
        assert_eq!(
            error.message,
            "recursive module reference from `direct` through `direct`"
        );

        let first = parse(
            "first.sv",
            r#"module first(input logic a, output logic y);
  second to_second(.a(a), .y(y));
endmodule
"#,
        );
        let second = parse(
            "second.sv",
            r#"module second(input logic a, output logic y);
  first to_first(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[first.clone(), second]).unwrap();
        let error = lower_design_with_catalog(&first, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("first.sv", 2, 3));
        assert_eq!(
            error.message,
            "recursive module reference from `first` through `second`"
        );
    }

    #[test]
    fn binding_and_connection_errors_keep_the_analyzer_diagnostics_and_spans() {
        let child = parse(
            "child.sv",
            r#"module child #(parameter real P = 1) (input logic a, output logic y);
  assign y = a;
endmodule
"#,
        );
        let bad_parameter = parse(
            "bad_parameter.sv",
            r#"module bad_parameter(input logic a, output logic y);
  child #(.NOPE(2)) u(.a(a), .y(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[bad_parameter.clone(), child.clone()]).unwrap();
        let error = lower_design_with_catalog(&bad_parameter, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("bad_parameter.sv", 2, 11));
        assert_eq!(error.message, "unknown parameter `NOPE` on module `child`");

        let bad_port = parse(
            "bad_port.sv",
            r#"module bad_port(input logic a, output logic y);
  child u(.a(a), .missing(y));
endmodule
"#,
        );
        let catalog = ModuleCatalog::from_designs(&[bad_port.clone(), child]).unwrap();
        let error = lower_design_with_catalog(&bad_port, &catalog).unwrap_err();
        assert_eq!(error.span, Span::new("bad_port.sv", 2, 18));
        assert_eq!(error.message, "unknown port `missing` on module `child`");
    }
}
