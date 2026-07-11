//! Typed, deterministic capability inventory for the curated cell corpus.
//!
//! A capability classification describes downstream conversion readiness and
//! roadmap ownership. It is deliberately independent of token frequency: an
//! often-observed construct can still be deferred or unsupported.

use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Milestone {
    M1,
    M2,
    M3,
    M4,
    M5,
    M6,
    M7,
    M8,
    M9,
    M10,
    M11,
    M12,
}

impl Milestone {
    pub fn label(self) -> &'static str {
        match self {
            Self::M1 => "M1 corpus inventory and lexer",
            Self::M2 => "M2 lossless specialized parser",
            Self::M3 => "M3 semantic analysis",
            Self::M4 => "M4 flat combinational SSA",
            Self::M5 => "M5 stateful procedural lowering",
            Self::M6 => "M6 tri-state and strength lowering",
            Self::M7 => "M7 symbolic timing",
            Self::M8 => "M8 generate selection",
            Self::M9 => "M9 hierarchy flattening",
            Self::M10 => "M10 keeper drivers",
            Self::M11 => "M11 transistor drivers",
            Self::M12 => "M12 release conversion",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Fully accepted by the converter's current verified end-to-end baseline.
    /// `owner` identifies the milestone that established that support.
    Supported { owner: Milestone },
    /// Recognized without approximation, but owned by a later roadmap milestone.
    Deferred { milestone: Milestone },
    /// Deliberately has no downstream representation for the stated contract reason.
    IntentionalIgnore { justification: String },
    /// Has no accepted downstream behavior or explicit ignore policy.
    Unsupported { reason: String },
}

impl Classification {
    pub fn label(&self) -> String {
        match self {
            Self::Supported { owner } => format!("supported({})", owner.label()),
            Self::Deferred { milestone } => format!("deferred({})", milestone.label()),
            Self::IntentionalIgnore { justification } => {
                format!("intentional-ignore({justification})")
            }
            Self::Unsupported { reason } => format!("unsupported({reason})"),
        }
    }

    pub fn kind(&self) -> ClassificationKind {
        match self {
            Self::Supported { .. } => ClassificationKind::Supported,
            Self::Deferred { .. } => ClassificationKind::Deferred,
            Self::IntentionalIgnore { .. } => ClassificationKind::IntentionalIgnore,
            Self::Unsupported { .. } => ClassificationKind::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClassificationKind {
    Supported,
    Deferred,
    IntentionalIgnore,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityFamily {
    Structure,
    Declaration,
    Statement,
    Expression,
    Procedural,
    Primitive,
    Hierarchy,
    Timing,
    Strength,
    Directive,
}

impl CapabilityFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Declaration => "declaration",
            Self::Statement => "statement",
            Self::Expression => "expression",
            Self::Procedural => "procedural",
            Self::Primitive => "primitive",
            Self::Hierarchy => "hierarchy",
            Self::Timing => "timing",
            Self::Strength => "strength",
            Self::Directive => "directive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemContext {
    Module,
    Procedural,
    Generate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprContext {
    Target,
    Value,
    Timing,
    Condition,
    GenerateCondition,
    Sensitivity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentContext {
    Initial,
    Procedural,
    Continuous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayContext {
    ContinuousAssign,
    Primitive,
    SpecifyPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemForm {
    Import,
    Declaration,
    Initial,
    ProceduralAssign,
    AlwaysLatch,
    Always,
    ContinuousAssign,
    Primitive,
    Instantiation,
    Specify,
    SpecifySpecparam,
    SpecifyPath,
    Generate,
    Block,
    If,
    Else,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprForm {
    Path,
    Integer,
    Real,
    Constant(ConstKind),
    Group,
    Unary,
    Binary,
    Ternary,
    Call,
    HighZUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitivityForm {
    Missing,
    Any,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideStyle {
    Named,
    Positional,
    PositionalOmitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStyle {
    Named,
    Positional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceKind {
    OrdinaryKnown,
    OrdinaryUnknown,
    Keeper,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Design,
    Module,
    Parameter(ParamKind),
    ParameterType(String),
    Port(Direction),
    PortModifier(String),
    Item {
        form: ItemForm,
        context: ItemContext,
    },
    Declaration(DeclKind),
    DeclarationType(String),
    Import {
        path: String,
        wildcard: bool,
    },
    Assignment {
        context: AssignmentContext,
        op: AssignOp,
    },
    InitialEvent {
        supported_literal: bool,
    },
    Always(AlwaysKind),
    Sensitivity(SensitivityForm),
    EventEdge(String),
    EventExpressionPresent(bool),
    Expression {
        context: ExprContext,
        form: ExprForm,
    },
    Unary {
        context: ExprContext,
        op: UnaryOp,
    },
    Binary {
        context: ExprContext,
        op: BinaryOp,
    },
    Call {
        context: ExprContext,
        name: String,
    },
    CallArguments {
        name: String,
        arity: usize,
        omitted: Vec<usize>,
    },
    Primitive(String),
    PrimitiveArguments {
        name: String,
        arity: usize,
        omitted: Vec<usize>,
    },
    Instantiation {
        kind: InstanceKind,
        module: String,
    },
    ParameterOverride(OverrideStyle),
    Connection(ConnectionStyle),
    StrengthPair(Vec<String>),
    DelayShape {
        context: DelayContext,
        arity: usize,
    },
    DelayOmission {
        context: DelayContext,
        index: usize,
    },
    DelayLaterEntries {
        context: DelayContext,
        arity: usize,
    },
    Directive(String),
}

impl Capability {
    pub fn family(&self) -> CapabilityFamily {
        match self {
            Self::Design | Self::Module => CapabilityFamily::Structure,
            Self::Parameter(_)
            | Self::ParameterType(_)
            | Self::Port(_)
            | Self::PortModifier(_)
            | Self::Declaration(_)
            | Self::DeclarationType(_) => CapabilityFamily::Declaration,
            Self::Item { .. }
            | Self::Assignment { .. }
            | Self::InitialEvent { .. }
            | Self::Import { .. } => CapabilityFamily::Statement,
            Self::Always(_)
            | Self::Sensitivity(_)
            | Self::EventEdge(_)
            | Self::EventExpressionPresent(_) => CapabilityFamily::Procedural,
            Self::Expression { .. }
            | Self::Unary { .. }
            | Self::Binary { .. }
            | Self::Call { .. }
            | Self::CallArguments { .. } => CapabilityFamily::Expression,
            Self::Primitive(_) | Self::PrimitiveArguments { .. } => CapabilityFamily::Primitive,
            Self::Instantiation { .. } | Self::ParameterOverride(_) | Self::Connection(_) => {
                CapabilityFamily::Hierarchy
            }
            Self::DelayShape { .. }
            | Self::DelayOmission { .. }
            | Self::DelayLaterEntries { .. } => CapabilityFamily::Timing,
            Self::StrengthPair(_) => CapabilityFamily::Strength,
            Self::Directive(_) => CapabilityFamily::Directive,
        }
    }

    pub fn id(&self) -> String {
        let detail = match self {
            Self::Design => "design".to_string(),
            Self::Module => "module".to_string(),
            Self::Parameter(kind) => format!("parameter.{}", param_kind_label(*kind)),
            Self::ParameterType(ty) => format!("parameter.type.{ty}"),
            Self::Port(direction) => format!("port.{}", direction_label(*direction)),
            Self::PortModifier(modifier) => format!("port.modifier.{modifier}"),
            Self::Item { form, context } => {
                format!(
                    "item.{}.{}",
                    item_context_label(*context),
                    item_form_label(*form)
                )
            }
            Self::Declaration(kind) => format!("signal.{}", decl_kind_label(*kind)),
            Self::DeclarationType(ty) => format!("signal.type.{ty}"),
            Self::Import { path, wildcard } => {
                format!(
                    "import.{path}.{}",
                    if *wildcard { "wildcard" } else { "exact" }
                )
            }
            Self::Assignment { context, op } => format!(
                "assignment.{}.{}",
                assignment_context_label(*context),
                assign_op_label(*op)
            ),
            Self::InitialEvent { supported_literal } => format!(
                "initial.event.{}",
                if *supported_literal {
                    "supported-literal"
                } else {
                    "unsupported-value"
                }
            ),
            Self::Always(kind) => format!("always.{}", always_kind_label(*kind)),
            Self::Sensitivity(form) => format!("sensitivity.{}", sensitivity_label(*form)),
            Self::EventEdge(edge) => format!("event.edge.{edge}"),
            Self::EventExpressionPresent(present) => format!(
                "event.expression.{}",
                if *present { "present" } else { "omitted" }
            ),
            Self::Expression { context, form } => {
                format!(
                    "{}.{}",
                    expr_context_label(*context),
                    expr_form_label(*form)
                )
            }
            Self::Unary { context, op } => format!(
                "unary.{}.{}",
                expr_context_label(*context),
                unary_op_label(*op)
            ),
            Self::Binary { context, op } => format!(
                "binary.{}.{}",
                expr_context_label(*context),
                binary_op_label(*op)
            ),
            Self::Call { context, name } => {
                format!("call.{}.{name}", expr_context_label(*context))
            }
            Self::CallArguments {
                name,
                arity,
                omitted,
            } => format!(
                "call.arguments.{name}.arity-{arity}.omitted-{}",
                index_list(omitted)
            ),
            Self::Primitive(name) => name.clone(),
            Self::PrimitiveArguments {
                name,
                arity,
                omitted,
            } => format!(
                "arguments.{name}.arity-{arity}.omitted-{}",
                index_list(omitted)
            ),
            Self::Instantiation { kind, module } => {
                format!("instantiation.{}.{module}", instance_kind_label(*kind))
            }
            Self::ParameterOverride(style) => {
                format!("instantiation.parameter.{}", override_style_label(*style))
            }
            Self::Connection(style) => {
                format!(
                    "instantiation.connection.{}",
                    connection_style_label(*style)
                )
            }
            Self::StrengthPair(values) => values.join("-"),
            Self::DelayShape { context, arity } => {
                format!("delay.{}.arity-{arity}", delay_context_label(*context))
            }
            Self::DelayOmission { context, index } => format!(
                "delay.{}.omitted-entry-{index}",
                delay_context_label(*context)
            ),
            Self::DelayLaterEntries { context, arity } => format!(
                "delay.{}.ignored-later-entries.arity-{arity}",
                delay_context_label(*context)
            ),
            Self::Directive(name) => name.clone(),
        };
        format!("{}.{}", self.family().label(), detail)
    }

    pub fn classification(&self) -> Classification {
        use Classification::{Deferred, IntentionalIgnore, Unsupported};
        match self {
            Self::Design | Self::Module => Deferred {
                milestone: Milestone::M2,
            },
            Self::Parameter(_) => Deferred {
                milestone: Milestone::M3,
            },
            Self::ParameterType(ty) | Self::DeclarationType(ty)
                if matches!(ty.as_str(), "real" | "realtime" | "logic" | "tri" | "wire") =>
            {
                Deferred {
                    milestone: Milestone::M3,
                }
            }
            Self::ParameterType(ty) | Self::DeclarationType(ty) => Unsupported {
                reason: format!("unknown scalar declaration type `{ty}`"),
            },
            Self::Port(_) => Deferred {
                milestone: Milestone::M3,
            },
            Self::PortModifier(modifier)
                if matches!(modifier.as_str(), "logic" | "tri" | "wire" | "real") =>
            {
                Deferred {
                    milestone: Milestone::M3,
                }
            }
            Self::PortModifier(modifier) => Unsupported {
                reason: format!("unsupported port modifier `{modifier}`"),
            },
            Self::Item { form, context } => classify_item(*form, *context),
            Self::Declaration(_) => Deferred {
                milestone: Milestone::M3,
            },
            Self::Import { path, wildcard: true }
                if matches!(path.as_str(), "dmg_timing" | "sm83_timing") =>
            {
                IntentionalIgnore {
                    justification:
                        "CONTRACT.md: behavior-free curated timing package import after resolution"
                            .to_string(),
                }
            }
            Self::Import { path, .. } => Unsupported {
                reason: format!("unrecognized import `{path}`"),
            },
            Self::Assignment { context, .. } => Deferred {
                milestone: match context {
                    AssignmentContext::Initial => Milestone::M3,
                    AssignmentContext::Procedural => Milestone::M5,
                    AssignmentContext::Continuous => Milestone::M4,
                },
            },
            Self::InitialEvent {
                supported_literal: true,
            } => IntentionalIgnore {
                justification:
                    "CONTRACT.md: literal initial value/event classifies state but is not serialized"
                        .to_string(),
            },
            Self::InitialEvent {
                supported_literal: false,
            } => Unsupported {
                reason: "initial value is not a contracted scalar literal".to_string(),
            },
            Self::Always(_) | Self::Sensitivity(_) => Deferred {
                milestone: Milestone::M3,
            },
            Self::EventEdge(edge) if matches!(edge.as_str(), "posedge" | "negedge") => Deferred {
                milestone: Milestone::M3,
            },
            Self::EventEdge(edge) => Unsupported {
                reason: format!("unknown event edge `{edge}`"),
            },
            Self::EventExpressionPresent(true) => Deferred {
                milestone: Milestone::M3,
            },
            Self::EventExpressionPresent(false) => Unsupported {
                reason: "event control omits its required expression".to_string(),
            },
            Self::Expression { context, form } => classify_expression(*context, *form),
            Self::Unary { context, op } => classify_unary(*context, *op),
            Self::Binary { context, op } => classify_binary(*context, *op),
            Self::Call { context, name } => classify_call(*context, name),
            Self::CallArguments {
                name,
                arity,
                omitted,
            } if is_known_call_shape(name, *arity, omitted) => Deferred {
                    milestone: Milestone::M7,
                },
            Self::CallArguments {
                name,
                arity,
                omitted,
            } => Unsupported {
                reason: format!(
                    "call `{name}` has unsupported arity {arity} with omitted arguments {}",
                    index_list(omitted)
                ),
            },
            Self::Primitive(name) => classify_primitive(name),
            Self::PrimitiveArguments {
                name,
                arity: 3,
                omitted,
            } if omitted.is_empty()
                && matches!(name.as_str(), "bufif0" | "bufif1" | "nmos" | "pmos" | "rnmos") =>
            {
                Deferred {
                    milestone: primitive_milestone(name),
                }
            }
            Self::PrimitiveArguments {
                name,
                arity,
                omitted,
            } => Unsupported {
                reason: format!(
                    "primitive `{name}` has arity {arity} with omitted arguments {}",
                    index_list(omitted)
                ),
            },
            Self::Instantiation {
                kind: InstanceKind::Keeper,
                ..
            } => Deferred {
                milestone: Milestone::M10,
            },
            Self::Instantiation {
                kind: InstanceKind::OrdinaryKnown,
                ..
            }
            | Self::ParameterOverride(_)
            | Self::Connection(_) => Deferred {
                milestone: Milestone::M9,
            },
            Self::Instantiation {
                kind: InstanceKind::OrdinaryUnknown,
                module,
            } => Unsupported {
                reason: format!("instance refers to unknown module `{module}`"),
            },
            Self::StrengthPair(values) if is_known_strength(values) => Deferred {
                milestone: Milestone::M6,
            },
            Self::StrengthPair(values) => Unsupported {
                reason: format!("uncontracted strength pair `({})`", values.join(", ")),
            },
            Self::DelayShape { arity: 1..=3, .. } => Deferred {
                milestone: Milestone::M7,
            },
            Self::DelayShape { arity, .. } => Unsupported {
                reason: format!("unsupported delay tuple arity {arity}"),
            },
            Self::DelayOmission { index: 0, .. } => Unsupported {
                reason: "explicitly omitted first delay entry has no contracted meaning".to_string(),
            },
            Self::DelayOmission { index, .. } => IntentionalIgnore {
                justification: format!(
                    "CONTRACT.md single-delay policy intentionally ignores omitted later entry {index}"
                ),
            },
            Self::DelayLaterEntries { .. } => IntentionalIgnore {
                justification:
                    "CONTRACT.md single-delay policy selects entry zero and ignores later entries"
                        .to_string(),
            },
            Self::Directive(name) if name == "`default_nettype" => IntentionalIgnore {
                justification:
                    "CONTRACT.md: curated default_nettype directive does not affect cell behavior"
                        .to_string(),
            },
            Self::Directive(name) => Unsupported {
                reason: format!("unrecognized directive `{name}`"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRecord {
    pub capability: Capability,
    pub classification: Classification,
    pub occurrences: usize,
    pub files: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityInventory {
    records: BTreeMap<String, CapabilityRecord>,
}

impl CapabilityInventory {
    pub fn record(&mut self, capability: Capability, path: impl Into<String>) {
        let id = capability.id();
        let classification = capability.classification();
        let path = path.into();
        match self.records.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(CapabilityRecord {
                    capability,
                    classification,
                    occurrences: 1,
                    files: BTreeSet::from([path]),
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let record = entry.get_mut();
                assert_eq!(
                    record.capability, capability,
                    "distinct capabilities produced the same stable ID"
                );
                assert_eq!(record.classification, classification);
                record.occurrences += 1;
                record.files.insert(path);
            }
        }
    }

    pub fn records(&self) -> &BTreeMap<String, CapabilityRecord> {
        &self.records
    }

    pub fn record_by_id(&self, id: &str) -> Option<&CapabilityRecord> {
        self.records.get(id)
    }

    pub fn classification_count(&self, kind: ClassificationKind) -> usize {
        self.records
            .values()
            .filter(|record| record.classification.kind() == kind)
            .count()
    }

    pub fn unsupported_count(&self) -> usize {
        self.classification_count(ClassificationKind::Unsupported)
    }

    pub fn all_classified(&self) -> bool {
        self.records
            .values()
            .all(|record| record.classification == record.capability.classification())
    }
}

pub struct InventoryWalker<'a> {
    inventory: &'a mut CapabilityInventory,
    known_modules: &'a BTreeSet<String>,
    path: &'a str,
}

impl<'a> InventoryWalker<'a> {
    pub fn new(
        inventory: &'a mut CapabilityInventory,
        known_modules: &'a BTreeSet<String>,
        path: &'a str,
    ) -> Self {
        Self {
            inventory,
            known_modules,
            path,
        }
    }

    pub fn record_tokens(&mut self, tokens: &[Token]) {
        for token in tokens {
            if token.kind == TokenKind::Directive {
                self.record(Capability::Directive(token.lexeme.clone()));
            }
        }
    }

    pub fn record_design(&mut self, design: &Design) {
        self.record(Capability::Design);
        for module in &design.modules {
            self.record(Capability::Module);
            for parameter in &module.parameters {
                self.record_parameter(parameter);
            }
            for port in &module.ports {
                self.record(Capability::Port(port.direction));
                for modifier in &port.modifiers {
                    self.record(Capability::PortModifier(modifier.clone()));
                }
            }
            for item in &module.items {
                self.record_item(item, ItemContext::Module);
            }
        }
    }

    fn record(&mut self, capability: Capability) {
        self.inventory.record(capability, self.path);
    }

    fn record_parameter(&mut self, parameter: &ParamDecl) {
        self.record(Capability::Parameter(parameter.kind));
        if let Some(ty) = &parameter.ty {
            self.record(Capability::ParameterType(ty.clone()));
        }
        self.record_expr(&parameter.value, ExprContext::Timing);
    }

    fn record_item(&mut self, item: &Item, context: ItemContext) {
        match &item.kind {
            ItemKind::Import(import) => {
                self.record_item_form(ItemForm::Import, context);
                self.record(Capability::Import {
                    path: import.path.join("::"),
                    wildcard: import.wildcard,
                });
            }
            ItemKind::Decl(decl) => {
                self.record_item_form(ItemForm::Declaration, context);
                self.record(Capability::Declaration(decl.kind));
                if let Some(ty) = &decl.ty {
                    self.record(Capability::DeclarationType(ty.clone()));
                }
                if let Some(value) = &decl.value {
                    self.record_expr(value, ExprContext::Timing);
                }
            }
            ItemKind::Initial(assign) => {
                self.record_item_form(ItemForm::Initial, context);
                self.record(Capability::InitialEvent {
                    supported_literal: is_supported_initial_literal(&assign.value),
                });
                self.record_assignment(assign, AssignmentContext::Initial);
            }
            ItemKind::ProcAssign(assign) => {
                self.record_item_form(ItemForm::ProceduralAssign, context);
                self.record_assignment(assign, AssignmentContext::Procedural);
            }
            ItemKind::AlwaysLatch(always) => {
                self.record_item_form(ItemForm::AlwaysLatch, context);
                if let Some(condition) = &always.condition {
                    self.record_expr(condition, ExprContext::Condition);
                }
                self.record_item(&always.body, ItemContext::Procedural);
            }
            ItemKind::Always(always) => {
                self.record_item_form(ItemForm::Always, context);
                self.record(Capability::Always(always.kind));
                match &always.sensitivity {
                    None => self.record(Capability::Sensitivity(SensitivityForm::Missing)),
                    Some(Sensitivity::Any) => {
                        self.record(Capability::Sensitivity(SensitivityForm::Any));
                    }
                    Some(Sensitivity::List(events)) => {
                        self.record(Capability::Sensitivity(SensitivityForm::List));
                        for event in events {
                            if let Some(edge) = &event.edge {
                                self.record(Capability::EventEdge(edge.clone()));
                            }
                            self.record(Capability::EventExpressionPresent(event.expr.is_some()));
                            if let Some(expr) = &event.expr {
                                self.record_expr(expr, ExprContext::Sensitivity);
                            }
                        }
                    }
                }
                self.record_item(&always.body, ItemContext::Procedural);
            }
            ItemKind::Assign(assign) => {
                self.record_item_form(ItemForm::ContinuousAssign, context);
                self.record(Capability::Assignment {
                    context: AssignmentContext::Continuous,
                    op: assign.op,
                });
                if let Some(strength) = &assign.strength {
                    self.record_strength(strength);
                }
                if let Some(delay) = &assign.delay {
                    self.record_delay(delay, DelayContext::ContinuousAssign);
                }
                self.record_expr(&assign.target, ExprContext::Target);
                self.record_expr(&assign.value, ExprContext::Value);
            }
            ItemKind::Primitive(primitive) => {
                self.record_item_form(ItemForm::Primitive, context);
                self.record(Capability::Primitive(primitive.name.clone()));
                let omitted = omitted_indices(&primitive.args);
                self.record(Capability::PrimitiveArguments {
                    name: primitive.name.clone(),
                    arity: primitive.args.len(),
                    omitted,
                });
                if let Some(strength) = &primitive.strength {
                    self.record_strength(strength);
                }
                if let Some(delay) = &primitive.delay {
                    self.record_delay(delay, DelayContext::Primitive);
                }
                for expr in primitive.args.iter().flatten() {
                    self.record_expr(expr, ExprContext::Value);
                }
            }
            ItemKind::Instantiation(instance) => {
                self.record_item_form(ItemForm::Instantiation, context);
                let kind = if instance.module == "keeper" {
                    InstanceKind::Keeper
                } else if self.known_modules.contains(&instance.module) {
                    InstanceKind::OrdinaryKnown
                } else {
                    InstanceKind::OrdinaryUnknown
                };
                self.record(Capability::Instantiation {
                    kind,
                    module: instance.module.clone(),
                });
                for parameter in &instance.parameters {
                    match parameter {
                        ParamOverride::Named { value, .. } => {
                            self.record(Capability::ParameterOverride(OverrideStyle::Named));
                            self.record_expr(value, ExprContext::Timing);
                        }
                        ParamOverride::Positional(Some(value)) => {
                            self.record(Capability::ParameterOverride(OverrideStyle::Positional));
                            self.record_expr(value, ExprContext::Timing);
                        }
                        ParamOverride::Positional(None) => {
                            self.record(Capability::ParameterOverride(
                                OverrideStyle::PositionalOmitted,
                            ));
                        }
                    }
                }
                for connection in &instance.connections {
                    match connection {
                        Connection::Named { value, .. } => {
                            self.record(Capability::Connection(ConnectionStyle::Named));
                            self.record_expr(value, ExprContext::Value);
                        }
                        Connection::Positional(value) => {
                            self.record(Capability::Connection(ConnectionStyle::Positional));
                            self.record_expr(value, ExprContext::Value);
                        }
                    }
                }
            }
            ItemKind::Specify(specify) => {
                self.record_item_form(ItemForm::Specify, context);
                for specify_item in &specify.items {
                    match specify_item {
                        SpecifyItem::Specparam(parameter) => {
                            self.record_item_form(ItemForm::SpecifySpecparam, context);
                            self.record_parameter(parameter);
                        }
                        SpecifyItem::Path(path) => {
                            self.record_item_form(ItemForm::SpecifyPath, context);
                            for control in &path.controls {
                                self.record_expr(control, ExprContext::Sensitivity);
                            }
                            self.record_expr(&path.target, ExprContext::Target);
                            self.record_delay_values(&path.delays, DelayContext::SpecifyPath);
                        }
                    }
                }
            }
            ItemKind::Generate(block) => {
                self.record_item_form(ItemForm::Generate, context);
                for nested in &block.items {
                    self.record_item(nested, ItemContext::Generate);
                }
            }
            ItemKind::Block(block) => {
                self.record_item_form(ItemForm::Block, context);
                for nested in &block.items {
                    self.record_item(nested, context);
                }
            }
            ItemKind::If(if_stmt) => {
                self.record_item_form(ItemForm::If, context);
                let expr_context = if context == ItemContext::Generate {
                    ExprContext::GenerateCondition
                } else {
                    ExprContext::Condition
                };
                self.record_expr(&if_stmt.condition, expr_context);
                self.record_item(&if_stmt.then_branch, context);
                if let Some(else_branch) = &if_stmt.else_branch {
                    self.record_item_form(ItemForm::Else, context);
                    self.record_item(else_branch, context);
                }
            }
            ItemKind::Empty => self.record_item_form(ItemForm::Empty, context),
        }
    }

    fn record_item_form(&mut self, form: ItemForm, context: ItemContext) {
        self.record(Capability::Item { form, context });
    }

    fn record_assignment(&mut self, assignment: &AssignStmt, context: AssignmentContext) {
        self.record(Capability::Assignment {
            context,
            op: assignment.op,
        });
        self.record_expr(&assignment.target, ExprContext::Target);
        self.record_expr(&assignment.value, ExprContext::Value);
    }

    fn record_strength(&mut self, strength: &Strength) {
        self.record(Capability::StrengthPair(strength.values.clone()));
    }

    fn record_delay(&mut self, delay: &Delay, context: DelayContext) {
        self.record_delay_values(&delay.values, context);
    }

    fn record_delay_values(&mut self, values: &[Option<Expr>], context: DelayContext) {
        self.record(Capability::DelayShape {
            context,
            arity: values.len(),
        });
        if values.len() > 1 {
            self.record(Capability::DelayLaterEntries {
                context,
                arity: values.len(),
            });
        }
        for (index, value) in values.iter().enumerate() {
            if let Some(value) = value {
                self.record_expr(value, ExprContext::Timing);
            } else {
                self.record(Capability::DelayOmission { context, index });
            }
        }
    }

    fn record_expr(&mut self, expr: &Expr, context: ExprContext) {
        let form = match &expr.kind {
            ExprKind::Path(_) => ExprForm::Path,
            ExprKind::Integer(_) => ExprForm::Integer,
            ExprKind::Real(_) => ExprForm::Real,
            ExprKind::Constant(kind) => ExprForm::Constant(*kind),
            ExprKind::Group(_) => ExprForm::Group,
            ExprKind::Unary { .. } => ExprForm::Unary,
            ExprKind::Binary { .. } => ExprForm::Binary,
            ExprKind::Ternary { .. } => ExprForm::Ternary,
            ExprKind::Call { .. } => ExprForm::Call,
        };
        self.record(Capability::Expression { context, form });
        match &expr.kind {
            ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) => {}
            ExprKind::Constant(kind) => {
                if *kind == ConstKind::Z {
                    self.record(Capability::Expression {
                        context,
                        form: ExprForm::HighZUse,
                    });
                }
            }
            ExprKind::Group(inner) => self.record_expr(inner, context),
            ExprKind::Unary { op, expr } => {
                self.record(Capability::Unary { context, op: *op });
                self.record_expr(expr, context);
            }
            ExprKind::Binary { op, left, right } => {
                self.record(Capability::Binary { context, op: *op });
                self.record_expr(left, context);
                self.record_expr(right, context);
            }
            ExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                let condition_context = if context == ExprContext::Timing {
                    ExprContext::Timing
                } else {
                    ExprContext::Condition
                };
                self.record_expr(condition, condition_context);
                self.record_expr(then_expr, context);
                self.record_expr(else_expr, context);
            }
            ExprKind::Call { callee, args } => {
                let name = match &callee.kind {
                    ExprKind::Path(path) => path.join("::"),
                    _ => "<expression>".to_string(),
                };
                self.record(Capability::Call {
                    context,
                    name: name.clone(),
                });
                self.record(Capability::CallArguments {
                    name,
                    arity: args.len(),
                    omitted: omitted_indices(args),
                });
                self.record_expr(callee, context);
                for arg in args.iter().flatten() {
                    self.record_expr(arg, context);
                }
            }
        }
    }
}

fn classify_item(form: ItemForm, context: ItemContext) -> Classification {
    let deferred = |milestone| Classification::Deferred { milestone };
    match (form, context) {
        (ItemForm::Generate, ItemContext::Module) | (_, ItemContext::Generate) => {
            deferred(Milestone::M8)
        }
        (
            ItemForm::ProceduralAssign | ItemForm::If | ItemForm::Else | ItemForm::Block,
            ItemContext::Procedural,
        ) => deferred(Milestone::M5),
        (ItemForm::Initial, ItemContext::Module) => deferred(Milestone::M3),
        (ItemForm::Always | ItemForm::AlwaysLatch, ItemContext::Module) => deferred(Milestone::M3),
        (ItemForm::ContinuousAssign, _) => deferred(Milestone::M4),
        (ItemForm::Primitive, _) => deferred(Milestone::M6),
        (ItemForm::Instantiation, _) => deferred(Milestone::M9),
        (ItemForm::Specify | ItemForm::SpecifySpecparam | ItemForm::SpecifyPath, _) => {
            deferred(Milestone::M7)
        }
        (ItemForm::Import | ItemForm::Declaration | ItemForm::Empty, ItemContext::Module) => {
            deferred(Milestone::M2)
        }
        (form, context) => Classification::Unsupported {
            reason: format!(
                "{} item is invalid in {} context",
                item_form_label(form),
                item_context_label(context)
            ),
        },
    }
}

fn classify_expression(context: ExprContext, form: ExprForm) -> Classification {
    use Classification::{Deferred, Unsupported};
    match context {
        ExprContext::GenerateCondition => Deferred {
            milestone: Milestone::M8,
        },
        ExprContext::Timing => match form {
            ExprForm::Constant(ConstKind::Z | ConstKind::X) | ExprForm::HighZUse => Unsupported {
                reason: "four-state literal is not a timing expression".to_string(),
            },
            _ => Deferred {
                milestone: Milestone::M7,
            },
        },
        ExprContext::Target | ExprContext::Sensitivity => Deferred {
            milestone: Milestone::M3,
        },
        ExprContext::Value | ExprContext::Condition => match form {
            ExprForm::Real | ExprForm::Call => Unsupported {
                reason: "real literals and calls are not contracted runtime values".to_string(),
            },
            ExprForm::Constant(ConstKind::Z) | ExprForm::HighZUse => Deferred {
                milestone: Milestone::M6,
            },
            _ => Deferred {
                milestone: Milestone::M4,
            },
        },
    }
}

fn classify_unary(context: ExprContext, op: UnaryOp) -> Classification {
    use Classification::{Deferred, Unsupported};
    match context {
        ExprContext::Timing => Deferred {
            milestone: Milestone::M7,
        },
        ExprContext::GenerateCondition => Deferred {
            milestone: Milestone::M8,
        },
        ExprContext::Value | ExprContext::Condition => match op {
            UnaryOp::Not | UnaryOp::BitNot => Deferred {
                milestone: Milestone::M4,
            },
            UnaryOp::Plus | UnaryOp::Minus => Unsupported {
                reason: "runtime unary arithmetic is not contracted".to_string(),
            },
        },
        ExprContext::Target | ExprContext::Sensitivity => Unsupported {
            reason: "unary expression is invalid in target/sensitivity context".to_string(),
        },
    }
}

fn classify_binary(context: ExprContext, op: BinaryOp) -> Classification {
    use Classification::{Deferred, Unsupported};
    match context {
        ExprContext::Timing => match op {
            BinaryOp::Less => Unsupported {
                reason: "timing less-than is not contracted".to_string(),
            },
            _ => Deferred {
                milestone: Milestone::M7,
            },
        },
        ExprContext::GenerateCondition => Deferred {
            milestone: Milestone::M8,
        },
        ExprContext::Value | ExprContext::Condition => match op {
            BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Less
            | BinaryOp::Greater => Unsupported {
                reason: "runtime arithmetic/ordering operator is not contracted".to_string(),
            },
            _ => Deferred {
                milestone: Milestone::M4,
            },
        },
        ExprContext::Target | ExprContext::Sensitivity => Unsupported {
            reason: "binary expression is invalid in target/sensitivity context".to_string(),
        },
    }
}

fn classify_call(context: ExprContext, name: &str) -> Classification {
    if context == ExprContext::Timing
        && matches!(name, "tpd_elmore" | "tpd_z" | "R_pmos_ohm" | "R_nmos_ohm")
    {
        Classification::Deferred {
            milestone: Milestone::M7,
        }
    } else {
        Classification::Unsupported {
            reason: format!(
                "call `{name}` is not contracted in {} context",
                expr_context_label(context)
            ),
        }
    }
}

fn classify_primitive(name: &str) -> Classification {
    if matches!(name, "bufif0" | "bufif1" | "nmos" | "pmos" | "rnmos") {
        Classification::Deferred {
            milestone: primitive_milestone(name),
        }
    } else {
        Classification::Unsupported {
            reason: format!("unknown primitive `{name}`"),
        }
    }
}

fn primitive_milestone(name: &str) -> Milestone {
    match name {
        "bufif0" | "bufif1" => Milestone::M6,
        "nmos" | "pmos" | "rnmos" => Milestone::M11,
        _ => Milestone::M1,
    }
}

fn is_known_strength(values: &[String]) -> bool {
    matches!(
        values,
        [first, second]
            if matches!(
                (first.as_str(), second.as_str()),
                ("strong1", "highz0")
                    | ("highz1", "strong0")
                    | ("pull1", "highz0")
                    | ("supply1", "supply0")
            )
    )
}

fn is_known_call_shape(name: &str, arity: usize, omitted: &[usize]) -> bool {
    match name {
        "tpd_elmore" => arity == 2 && omitted.is_empty(),
        "R_pmos_ohm" | "R_nmos_ohm" => arity == 1 && omitted.is_empty(),
        "tpd_z" => matches!(arity, 1 | 2) && omitted.len() < arity,
        _ => false,
    }
}

fn is_supported_initial_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Constant(_) => true,
        ExprKind::Integer(value) => matches!(value.as_str(), "0" | "1"),
        ExprKind::Group(inner) => is_supported_initial_literal(inner),
        _ => false,
    }
}

fn omitted_indices<T>(values: &[Option<T>]) -> Vec<usize> {
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.is_none().then_some(index))
        .collect()
}

fn index_list(indices: &[usize]) -> String {
    if indices.is_empty() {
        "none".to_string()
    } else {
        indices
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("-")
    }
}

fn param_kind_label(kind: ParamKind) -> &'static str {
    match kind {
        ParamKind::Parameter => "parameter",
        ParamKind::Localparam => "localparam",
        ParamKind::Specparam => "specparam",
    }
}

fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Input => "input",
        Direction::Output => "output",
        Direction::Inout => "inout",
    }
}

fn decl_kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Logic => "logic",
        DeclKind::Tri => "tri",
        DeclKind::Wire => "wire",
        DeclKind::Parameter => "parameter",
        DeclKind::Localparam => "localparam",
        DeclKind::Specparam => "specparam",
    }
}

fn item_context_label(context: ItemContext) -> &'static str {
    match context {
        ItemContext::Module => "module",
        ItemContext::Procedural => "procedural",
        ItemContext::Generate => "generate",
    }
}

fn item_form_label(form: ItemForm) -> &'static str {
    match form {
        ItemForm::Import => "import",
        ItemForm::Declaration => "declaration",
        ItemForm::Initial => "initial",
        ItemForm::ProceduralAssign => "procedural-assignment",
        ItemForm::AlwaysLatch => "always-latch",
        ItemForm::Always => "always",
        ItemForm::ContinuousAssign => "continuous-assign",
        ItemForm::Primitive => "primitive",
        ItemForm::Instantiation => "instantiation",
        ItemForm::Specify => "specify",
        ItemForm::SpecifySpecparam => "specify-specparam",
        ItemForm::SpecifyPath => "specify-path",
        ItemForm::Generate => "generate",
        ItemForm::Block => "block",
        ItemForm::If => "if",
        ItemForm::Else => "else",
        ItemForm::Empty => "empty",
    }
}

fn assignment_context_label(context: AssignmentContext) -> &'static str {
    match context {
        AssignmentContext::Initial => "initial",
        AssignmentContext::Procedural => "procedural",
        AssignmentContext::Continuous => "continuous",
    }
}

fn assign_op_label(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Blocking => "blocking",
        AssignOp::NonBlocking => "nonblocking",
    }
}

fn always_kind_label(kind: AlwaysKind) -> &'static str {
    match kind {
        AlwaysKind::Plain => "plain",
        AlwaysKind::Comb => "comb",
        AlwaysKind::Ff => "ff",
    }
}

fn sensitivity_label(form: SensitivityForm) -> &'static str {
    match form {
        SensitivityForm::Missing => "missing",
        SensitivityForm::Any => "any",
        SensitivityForm::List => "list",
    }
}

fn expr_context_label(context: ExprContext) -> &'static str {
    match context {
        ExprContext::Target => "target",
        ExprContext::Value => "value",
        ExprContext::Timing => "timing",
        ExprContext::Condition => "condition",
        ExprContext::GenerateCondition => "generate-condition",
        ExprContext::Sensitivity => "sensitivity",
    }
}

fn expr_form_label(form: ExprForm) -> &'static str {
    match form {
        ExprForm::Path => "path",
        ExprForm::Integer => "integer",
        ExprForm::Real => "real",
        ExprForm::Constant(kind) => match kind {
            ConstKind::Zero => "constant-zero",
            ConstKind::One => "constant-one",
            ConstKind::Z => "constant-z",
            ConstKind::X => "constant-x",
        },
        ExprForm::Group => "group",
        ExprForm::Unary => "unary",
        ExprForm::Binary => "binary",
        ExprForm::Ternary => "ternary",
        ExprForm::Call => "call",
        ExprForm::HighZUse => "high-z-use",
    }
}

fn unary_op_label(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bit-not",
        UnaryOp::Plus => "plus",
        UnaryOp::Minus => "minus",
    }
}

fn binary_op_label(op: BinaryOp) -> &'static str {
    match op {
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
    }
}

fn instance_kind_label(kind: InstanceKind) -> &'static str {
    match kind {
        InstanceKind::OrdinaryKnown => "ordinary-known",
        InstanceKind::OrdinaryUnknown => "ordinary-unknown",
        InstanceKind::Keeper => "keeper",
    }
}

fn override_style_label(style: OverrideStyle) -> &'static str {
    match style {
        OverrideStyle::Named => "named",
        OverrideStyle::Positional => "positional",
        OverrideStyle::PositionalOmitted => "positional-omitted",
    }
}

fn connection_style_label(style: ConnectionStyle) -> &'static str {
    match style {
        ConnectionStyle::Named => "named",
        ConnectionStyle::Positional => "positional",
    }
}

fn delay_context_label(context: DelayContext) -> &'static str {
    match context {
        DelayContext::ContinuousAssign => "continuous-assign",
        DelayContext::Primitive => "primitive",
        DelayContext::SpecifyPath => "specify-path",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex_file;
    use crate::parser::parse_file;
    use std::path::Path;

    fn inventory_for(source: &str, known_modules: &[&str]) -> CapabilityInventory {
        let path = Path::new("focused.sv");
        let tokens = lex_file(path, source).unwrap();
        let design = parse_file(path, source).unwrap();
        let known_modules = known_modules
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>();
        let mut inventory = CapabilityInventory::default();
        let mut walker = InventoryWalker::new(&mut inventory, &known_modules, "focused.sv");
        walker.record_tokens(&tokens);
        walker.record_design(&design);
        inventory
    }

    fn assert_file(inventory: &CapabilityInventory, id: &str) {
        let record = inventory
            .record_by_id(id)
            .unwrap_or_else(|| panic!("missing capability `{id}`"));
        assert_eq!(record.files, BTreeSet::from(["focused.sv".to_string()]));
    }

    #[test]
    fn walks_nested_items_and_expression_contexts_with_file_attribution() {
        let source = r#"
`default_nettype none
module child(input logic a, output logic y);
  assign y = a;
endmodule
module top #(parameter real T = 1.0) (
  input logic a, b, clk,
  output tri logic y
);
  import dmg_timing::*;
  logic q;
  localparam realtime D = tpd_elmore(T, R_pmos_ohm(2*T));
  initial q = '0;
  generate
    if (nodelay) begin
      assign #(D, D, D) y = a ? b : 'z;
    end else begin
      bufif0 (strong1, highz0) #(D) (y, '1, a);
    end
  endgenerate
  always_ff @(posedge clk, negedge a) q <= a & b;
  child child_inst(.a(q), .y(y));
  specify
    specparam T_y = D;
    (a *> y) = (T_y, T_y);
  endspecify
endmodule
"#;
        let inventory = inventory_for(source, &["child", "top"]);

        for id in [
            "directive.`default_nettype",
            "statement.item.module.generate",
            "statement.item.generate.if",
            "expression.generate-condition.path",
            "expression.value.ternary",
            "expression.value.high-z-use",
            "expression.timing.call",
            "expression.call.timing.tpd_elmore",
            "procedural.always.ff",
            "procedural.event.edge.posedge",
            "hierarchy.instantiation.ordinary-known.child",
            "hierarchy.instantiation.connection.named",
            "strength.strong1-highz0",
            "timing.delay.primitive.arity-1",
            "timing.delay.continuous-assign.arity-3",
            "statement.item.module.specify-path",
        ] {
            assert_file(&inventory, id);
        }
        assert!(inventory.all_classified());
        assert_eq!(inventory.unsupported_count(), 0);
    }

    #[test]
    fn unknown_dynamic_forms_are_explicitly_unsupported() {
        let source = r#"
`mystery setting
module bad(input logic a, b, output logic y);
  initial y = a;
  mystery (weak1, strong0) #(T) (y, a, b);
  alien alien_inst(.x(a));
endmodule
"#;
        let inventory = inventory_for(source, &["bad"]);

        for id in [
            "directive.`mystery",
            "statement.initial.event.unsupported-value",
            "primitive.mystery",
            "primitive.arguments.mystery.arity-3.omitted-none",
            "strength.weak1-strong0",
            "hierarchy.instantiation.ordinary-unknown.alien",
        ] {
            let record = inventory
                .record_by_id(id)
                .unwrap_or_else(|| panic!("missing unsupported capability `{id}`"));
            assert!(matches!(
                record.classification,
                Classification::Unsupported { .. }
            ));
            assert_eq!(record.files, BTreeSet::from(["focused.sv".to_string()]));
        }
        assert_eq!(inventory.unsupported_count(), 6);
        assert!(inventory.all_classified());
    }

    #[test]
    fn catalog_classifies_every_static_ast_variant_without_id_collisions() {
        let mut capabilities = vec![Capability::Design, Capability::Module];
        capabilities.extend(
            [
                ParamKind::Parameter,
                ParamKind::Localparam,
                ParamKind::Specparam,
            ]
            .into_iter()
            .map(Capability::Parameter),
        );
        capabilities.extend(
            [Direction::Input, Direction::Output, Direction::Inout]
                .into_iter()
                .map(Capability::Port),
        );
        capabilities.extend(
            [
                DeclKind::Logic,
                DeclKind::Tri,
                DeclKind::Wire,
                DeclKind::Parameter,
                DeclKind::Localparam,
                DeclKind::Specparam,
            ]
            .into_iter()
            .map(Capability::Declaration),
        );
        capabilities.extend(
            [
                ItemForm::Import,
                ItemForm::Declaration,
                ItemForm::Initial,
                ItemForm::ProceduralAssign,
                ItemForm::AlwaysLatch,
                ItemForm::Always,
                ItemForm::ContinuousAssign,
                ItemForm::Primitive,
                ItemForm::Instantiation,
                ItemForm::Specify,
                ItemForm::SpecifySpecparam,
                ItemForm::SpecifyPath,
                ItemForm::Generate,
                ItemForm::Block,
                ItemForm::If,
                ItemForm::Else,
                ItemForm::Empty,
            ]
            .into_iter()
            .map(|form| Capability::Item {
                form,
                context: ItemContext::Module,
            }),
        );
        capabilities.extend(
            [AlwaysKind::Plain, AlwaysKind::Comb, AlwaysKind::Ff]
                .into_iter()
                .map(Capability::Always),
        );
        capabilities.extend(
            [
                SensitivityForm::Missing,
                SensitivityForm::Any,
                SensitivityForm::List,
            ]
            .into_iter()
            .map(Capability::Sensitivity),
        );
        capabilities.extend(
            [
                ExprContext::Target,
                ExprContext::Value,
                ExprContext::Timing,
                ExprContext::Condition,
                ExprContext::GenerateCondition,
                ExprContext::Sensitivity,
            ]
            .into_iter()
            .map(|context| Capability::Expression {
                context,
                form: ExprForm::Path,
            }),
        );
        capabilities.extend(
            [
                ExprForm::Integer,
                ExprForm::Real,
                ExprForm::Constant(ConstKind::Zero),
                ExprForm::Constant(ConstKind::One),
                ExprForm::Constant(ConstKind::Z),
                ExprForm::Constant(ConstKind::X),
                ExprForm::Group,
                ExprForm::Unary,
                ExprForm::Binary,
                ExprForm::Ternary,
                ExprForm::Call,
                ExprForm::HighZUse,
            ]
            .into_iter()
            .map(|form| Capability::Expression {
                context: ExprContext::Value,
                form,
            }),
        );
        capabilities.extend(
            [UnaryOp::Not, UnaryOp::BitNot, UnaryOp::Plus, UnaryOp::Minus]
                .into_iter()
                .map(|op| Capability::Unary {
                    context: ExprContext::Timing,
                    op,
                }),
        );
        capabilities.extend(
            [
                BinaryOp::Mul,
                BinaryOp::Div,
                BinaryOp::Add,
                BinaryOp::Sub,
                BinaryOp::BitAnd,
                BinaryOp::BitOr,
                BinaryOp::BitXor,
                BinaryOp::BitNand,
                BinaryOp::BitNor,
                BinaryOp::BitXnor,
                BinaryOp::LogicalAnd,
                BinaryOp::LogicalOr,
                BinaryOp::Eq,
                BinaryOp::CaseEq,
                BinaryOp::Neq,
                BinaryOp::CaseNeq,
                BinaryOp::Less,
                BinaryOp::Greater,
            ]
            .into_iter()
            .map(|op| Capability::Binary {
                context: ExprContext::Timing,
                op,
            }),
        );
        capabilities.extend(
            [AssignOp::Blocking, AssignOp::NonBlocking]
                .into_iter()
                .map(|op| Capability::Assignment {
                    context: AssignmentContext::Procedural,
                    op,
                }),
        );
        capabilities.extend([
            Capability::Assignment {
                context: AssignmentContext::Initial,
                op: AssignOp::Blocking,
            },
            Capability::Assignment {
                context: AssignmentContext::Continuous,
                op: AssignOp::Blocking,
            },
            Capability::Primitive("bufif0".to_string()),
            Capability::Instantiation {
                kind: InstanceKind::OrdinaryKnown,
                module: "known".to_string(),
            },
            Capability::Instantiation {
                kind: InstanceKind::OrdinaryUnknown,
                module: "unknown".to_string(),
            },
            Capability::Instantiation {
                kind: InstanceKind::Keeper,
                module: "keeper".to_string(),
            },
            Capability::ParameterOverride(OverrideStyle::Named),
            Capability::ParameterOverride(OverrideStyle::Positional),
            Capability::ParameterOverride(OverrideStyle::PositionalOmitted),
            Capability::Connection(ConnectionStyle::Named),
            Capability::Connection(ConnectionStyle::Positional),
            Capability::StrengthPair(vec!["strong1".to_string(), "highz0".to_string()]),
            Capability::DelayShape {
                context: DelayContext::ContinuousAssign,
                arity: 1,
            },
            Capability::DelayShape {
                context: DelayContext::Primitive,
                arity: 2,
            },
            Capability::DelayShape {
                context: DelayContext::SpecifyPath,
                arity: 3,
            },
            Capability::Directive("`default_nettype".to_string()),
        ]);

        let mut ids = BTreeSet::new();
        let mut families = BTreeSet::new();
        for capability in capabilities {
            assert!(ids.insert(capability.id()), "duplicate capability ID");
            families.insert(capability.family());
            let _classification = capability.classification();
        }
        assert_eq!(
            families,
            BTreeSet::from([
                CapabilityFamily::Structure,
                CapabilityFamily::Declaration,
                CapabilityFamily::Statement,
                CapabilityFamily::Expression,
                CapabilityFamily::Procedural,
                CapabilityFamily::Primitive,
                CapabilityFamily::Hierarchy,
                CapabilityFamily::Timing,
                CapabilityFamily::Strength,
                CapabilityFamily::Directive,
            ])
        );
    }
}
