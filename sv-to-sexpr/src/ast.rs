use crate::diagnostic::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Design {
    pub modules: Vec<Module>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub span: Span,
    pub name: String,
    pub parameters: Vec<ParamDecl>,
    pub ports: Vec<PortDecl>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    pub span: Span,
    pub kind: ParamKind,
    pub ty: Option<String>,
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Parameter,
    Localparam,
    Specparam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDecl {
    pub span: Span,
    pub direction: Direction,
    pub modifiers: Vec<String>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    Inout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub span: Span,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    Import(ImportDecl),
    Decl(Decl),
    Initial(AssignStmt),
    ProcAssign(AssignStmt),
    AlwaysLatch(AlwaysLatch),
    Always(AlwaysBlock),
    Assign(AssignDecl),
    Primitive(PrimitiveCall),
    Instantiation(Instantiation),
    Specify(SpecifyBlock),
    Generate(Block),
    Block(Block),
    If(IfStmt),
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDecl {
    pub span: Span,
    pub path: Vec<String>,
    pub wildcard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub span: Span,
    pub kind: DeclKind,
    pub ty: Option<String>,
    pub names: Vec<String>,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Logic,
    Tri,
    Wire,
    Parameter,
    Localparam,
    Specparam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignDecl {
    pub span: Span,
    pub strength: Option<Strength>,
    pub delay: Option<Delay>,
    pub target: Expr,
    pub value: Expr,
    pub op: AssignOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Blocking,
    NonBlocking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignStmt {
    pub span: Span,
    pub target: Expr,
    pub value: Expr,
    pub op: AssignOp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlwaysLatch {
    pub span: Span,
    pub condition: Option<Expr>,
    pub body: Box<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlwaysBlock {
    pub span: Span,
    pub kind: AlwaysKind,
    pub sensitivity: Option<Sensitivity>,
    pub body: Box<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysKind {
    Plain,
    Comb,
    Ff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sensitivity {
    Any,
    List(Vec<EventControl>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventControl {
    pub span: Span,
    pub edge: Option<String>,
    pub expr: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimitiveCall {
    pub span: Span,
    pub name: String,
    pub strength: Option<Strength>,
    pub delay: Option<Delay>,
    pub args: Vec<Option<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instantiation {
    pub span: Span,
    pub module: String,
    pub parameters: Vec<ParamOverride>,
    pub instance: String,
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamOverride {
    Named { name: String, value: Expr },
    Positional(Option<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Named { name: String, value: Expr },
    Positional(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strength {
    pub span: Span,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delay {
    pub span: Span,
    pub values: Vec<Option<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub span: Span,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStmt {
    pub span: Span,
    pub condition: Expr,
    pub then_branch: Box<Item>,
    pub else_branch: Option<Box<Item>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecifyBlock {
    pub span: Span,
    pub items: Vec<SpecifyItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecifyItem {
    Specparam(ParamDecl),
    Path(SpecPath),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecPath {
    pub span: Span,
    pub controls: Vec<Expr>,
    pub target: Expr,
    pub delays: Vec<Option<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Path(Vec<String>),
    Integer(String),
    Real(String),
    Constant(ConstKind),
    Group(Box<Expr>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Ternary {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Option<Expr>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    Zero,
    One,
    Z,
    X,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    BitNot,
    Plus,
    Minus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Mul,
    Div,
    Add,
    Sub,
    BitAnd,
    BitOr,
    BitXor,
    BitNand,
    BitNor,
    BitXnor,
    LogicalAnd,
    LogicalOr,
    Eq,
    CaseEq,
    Neq,
    CaseNeq,
    Less,
    Greater,
}
