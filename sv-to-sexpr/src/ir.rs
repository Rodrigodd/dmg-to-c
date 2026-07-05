use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub registers: Vec<String>,
    pub items: Vec<CellItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellItem {
    Blank,
    Comment(String),
    Assignment(Assignment),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub target: String,
    pub expr: Expr,
    pub delay: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Atom(String),
    List(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredModule {
    pub cell: Cell,
    pub timing_aliases: BTreeMap<String, Expr>,
}

impl Expr {
    pub fn atom(value: impl Into<String>) -> Self {
        Self::Atom(value.into())
    }

    pub fn list(items: Vec<Expr>) -> Self {
        Self::List(items)
    }
}
