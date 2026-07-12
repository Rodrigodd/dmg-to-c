use std::collections::BTreeMap;
use std::fmt;

use crate::diagnostic::Diagnostic;

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
    /// Non-failing source diagnostics produced while constructing this cell.
    ///
    /// Diagnostics are deliberately kept outside [`Cell`], so they can be
    /// surfaced by commands and reports without becoming serialized DSL data.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueOperator {
    Not,
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
    Mux,
    BufIf0,
    BufIf1,
    DriveStrength,
    BufIf0Strength,
    BufIf1Strength,
    Eq,
    CaseEq,
    Neq,
    CaseNeq,
    Keeper,
    Nmos,
    Pmos,
    Rnmos,
}

impl ValueOperator {
    pub const ALL: [Self; 21] = [
        Self::Not,
        Self::And,
        Self::Or,
        Self::Xor,
        Self::Nand,
        Self::Nor,
        Self::Xnor,
        Self::Mux,
        Self::BufIf0,
        Self::BufIf1,
        Self::DriveStrength,
        Self::BufIf0Strength,
        Self::BufIf1Strength,
        Self::Eq,
        Self::CaseEq,
        Self::Neq,
        Self::CaseNeq,
        Self::Keeper,
        Self::Nmos,
        Self::Pmos,
        Self::Rnmos,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Nand => "nand",
            Self::Nor => "nor",
            Self::Xnor => "xnor",
            Self::Mux => "mux",
            Self::BufIf0 => "bufif0",
            Self::BufIf1 => "bufif1",
            Self::DriveStrength => "drive-strength",
            Self::BufIf0Strength => "bufif0-strength",
            Self::BufIf1Strength => "bufif1-strength",
            Self::Eq => "eq",
            Self::CaseEq => "caseeq",
            Self::Neq => "neq",
            Self::CaseNeq => "caseneq",
            Self::Keeper => "keeper",
            Self::Nmos => "nmos",
            Self::Pmos => "pmos",
            Self::Rnmos => "rnmos",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operator| operator.as_str() == value)
    }

    pub const fn accepts_arity(self, arity: usize) -> bool {
        match self {
            Self::Not => arity == 1,
            Self::Keeper => arity == 0,
            Self::And | Self::Or | Self::Xor | Self::Nand | Self::Nor | Self::Xnor => arity >= 2,
            Self::Mux => arity == 3,
            Self::DriveStrength => arity == 3,
            Self::BufIf0Strength | Self::BufIf1Strength => arity == 4,
            Self::BufIf0 | Self::BufIf1 | Self::Eq | Self::CaseEq | Self::Neq | Self::CaseNeq => {
                arity == 2
            }
            Self::Nmos | Self::Pmos | Self::Rnmos => arity == 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Elmore,
    Wire,
    Pmos,
    Nmos,
    Greater,
    Mux,
}

impl TimingOperator {
    pub const ALL: [Self; 10] = [
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::Elmore,
        Self::Wire,
        Self::Pmos,
        Self::Nmos,
        Self::Greater,
        Self::Mux,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Elmore => "elmore",
            Self::Wire => "wire",
            Self::Pmos => "pmos",
            Self::Nmos => "nmos",
            Self::Greater => "gt",
            Self::Mux => "mux",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operator| operator.as_str() == value)
    }

    pub const fn accepts_arity(self, arity: usize) -> bool {
        match self {
            Self::Add | Self::Multiply => arity >= 2,
            Self::Subtract | Self::Divide | Self::Elmore => arity == 2,
            Self::Wire | Self::Pmos | Self::Nmos => arity == 1,
            Self::Greater => arity == 2,
            Self::Mux => arity == 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub context: String,
    pub message: String,
}

impl ValidationError {
    fn new(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

impl std::error::Error for ValidationError {}

impl Cell {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.is_empty() {
            return Err(ValidationError::new("cell", "name must not be empty"));
        }
        validate_names("inputs", &self.inputs)?;
        validate_names("outputs", &self.outputs)?;
        validate_names("registers", &self.registers)?;
        for item in &self.items {
            if let CellItem::Assignment(assignment) = item {
                assignment.validate()?;
            }
        }
        Ok(())
    }
}

fn validate_names(context: &str, names: &[String]) -> Result<(), ValidationError> {
    if names.iter().any(String::is_empty) {
        return Err(ValidationError::new(
            context,
            "names must be non-empty atoms",
        ));
    }
    Ok(())
}

impl Assignment {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.target.is_empty() {
            return Err(ValidationError::new(
                "assignment",
                "target must not be empty",
            ));
        }
        self.expr
            .validate_value(&format!("assignment `{}` value", self.target))?;
        self.delay
            .validate_timing(&format!("assignment `{}` delay", self.target))
    }
}

impl Expr {
    pub fn atom(value: impl Into<String>) -> Self {
        Self::Atom(value.into())
    }

    pub fn list(items: Vec<Expr>) -> Self {
        Self::List(items)
    }

    pub fn value(operator: ValueOperator, operands: Vec<Expr>) -> Self {
        let mut items = Vec::with_capacity(operands.len() + 1);
        items.push(Self::atom(operator.as_str()));
        items.extend(operands);
        Self::list(items)
    }

    pub fn timing(operator: TimingOperator, operands: Vec<Expr>) -> Self {
        let mut items = Vec::with_capacity(operands.len() + 1);
        items.push(Self::atom(operator.as_str()));
        items.extend(operands);
        Self::list(items)
    }

    pub fn validate_value(&self, context: &str) -> Result<(), ValidationError> {
        match self {
            Self::Atom(atom) if atom == "z" => Err(ValidationError::new(
                context,
                "high-Z is legal only as the implicit disabled state of a driver",
            )),
            Self::Atom(atom) if !atom.is_empty() => Ok(()),
            Self::Atom(_) => Err(ValidationError::new(context, "atom must not be empty")),
            Self::List(items) => {
                let (head, operands) = split_operator(items, context)?;
                let operator = ValueOperator::parse(head).ok_or_else(|| {
                    ValidationError::new(context, format!("unknown value operator `{head}`"))
                })?;
                if !operator.accepts_arity(operands.len()) {
                    return Err(ValidationError::new(
                        context,
                        format!(
                            "wrong arity for value operator `{}`: got {}",
                            operator.as_str(),
                            operands.len()
                        ),
                    ));
                }
                for operand in operands {
                    let Self::Atom(atom) = operand else {
                        return Err(ValidationError::new(
                            context,
                            "value operator operands must be non-empty atoms",
                        ));
                    };
                    if atom.is_empty() {
                        return Err(ValidationError::new(
                            context,
                            "value operator operands must be non-empty atoms",
                        ));
                    }
                    if atom == "z"
                        && !matches!(
                            operator,
                            ValueOperator::Eq
                                | ValueOperator::CaseEq
                                | ValueOperator::Neq
                                | ValueOperator::CaseNeq
                        )
                    {
                        return Err(ValidationError::new(
                            context,
                            "high-Z may appear only in an equality operand",
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    pub fn validate_timing(&self, context: &str) -> Result<(), ValidationError> {
        match self {
            Self::Atom(atom) if !atom.is_empty() => Ok(()),
            Self::Atom(_) => Err(ValidationError::new(context, "atom must not be empty")),
            Self::List(items) => {
                let (head, operands) = split_operator(items, context)?;
                let operator = TimingOperator::parse(head).ok_or_else(|| {
                    ValidationError::new(context, format!("unknown timing operator `{head}`"))
                })?;
                if !operator.accepts_arity(operands.len()) {
                    return Err(ValidationError::new(
                        context,
                        format!(
                            "wrong arity for timing operator `{}`: got {}",
                            operator.as_str(),
                            operands.len()
                        ),
                    ));
                }
                for operand in operands {
                    operand.validate_timing(context)?;
                }
                Ok(())
            }
        }
    }
}

fn split_operator<'a>(
    items: &'a [Expr],
    context: &str,
) -> Result<(&'a str, &'a [Expr]), ValidationError> {
    let Some((head, operands)) = items.split_first() else {
        return Err(ValidationError::new(
            context,
            "operator list must not be empty",
        ));
    };
    let Expr::Atom(head) = head else {
        return Err(ValidationError::new(context, "operator must be an atom"));
    };
    if head.is_empty() {
        return Err(ValidationError::new(context, "operator must not be empty"));
    }
    Ok((head, operands))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(expr: Expr, delay: Expr) -> Assignment {
        Assignment {
            target: "y".to_string(),
            expr,
            delay,
        }
    }

    #[test]
    fn every_contracted_value_operator_validates() {
        for operator in ValueOperator::ALL {
            assert_eq!(ValueOperator::parse(operator.as_str()), Some(operator));
            let accepts = |arity| match operator {
                ValueOperator::Not => arity == 1,
                ValueOperator::Keeper => arity == 0,
                ValueOperator::And
                | ValueOperator::Or
                | ValueOperator::Xor
                | ValueOperator::Nand
                | ValueOperator::Nor
                | ValueOperator::Xnor => arity >= 2,
                ValueOperator::Mux | ValueOperator::DriveStrength => arity == 3,
                ValueOperator::BufIf0Strength | ValueOperator::BufIf1Strength => arity == 4,
                ValueOperator::BufIf0
                | ValueOperator::BufIf1
                | ValueOperator::Eq
                | ValueOperator::CaseEq
                | ValueOperator::Neq
                | ValueOperator::CaseNeq
                | ValueOperator::Nmos
                | ValueOperator::Pmos
                | ValueOperator::Rnmos => arity == 2,
            };
            for arity in 0..=5 {
                assert_eq!(
                    operator.accepts_arity(arity),
                    accepts(arity),
                    "{} with arity {arity}",
                    operator.as_str()
                );
            }
            let arity = (0..=5).find(|&arity| accepts(arity)).unwrap();
            Expr::value(
                operator,
                (0..arity)
                    .map(|index| Expr::atom(format!("a{index}")))
                    .collect(),
            )
            .validate_value("test")
            .unwrap();
        }
    }

    #[test]
    fn rejects_nested_unknown_and_wrong_arity_value_expressions() {
        let nested = Expr::value(
            ValueOperator::And,
            vec![
                Expr::value(ValueOperator::Not, vec![Expr::atom("a")]),
                Expr::atom("b"),
            ],
        );
        assert!(nested.validate_value("test").is_err());
        assert!(
            Expr::list(vec![Expr::atom("mystery"), Expr::atom("a")])
                .validate_value("test")
                .is_err()
        );
        assert!(
            Expr::value(ValueOperator::Mux, vec![Expr::atom("a"), Expr::atom("b")])
                .validate_value("test")
                .is_err()
        );
    }

    #[test]
    fn every_timing_operator_and_nested_delays_validate() {
        for operator in TimingOperator::ALL {
            assert_eq!(TimingOperator::parse(operator.as_str()), Some(operator));
            let accepts = |arity| match operator {
                TimingOperator::Add | TimingOperator::Multiply => arity >= 2,
                TimingOperator::Subtract | TimingOperator::Divide | TimingOperator::Elmore => {
                    arity == 2
                }
                TimingOperator::Wire | TimingOperator::Pmos | TimingOperator::Nmos => arity == 1,
                TimingOperator::Greater => arity == 2,
                TimingOperator::Mux => arity == 3,
            };
            for arity in 0..=4 {
                assert_eq!(
                    operator.accepts_arity(arity),
                    accepts(arity),
                    "{} with arity {arity}",
                    operator.as_str()
                );
            }
            let arity = (0..=4).find(|&arity| accepts(arity)).unwrap();
            Expr::timing(operator, vec![Expr::atom("1"); arity])
                .validate_timing("test")
                .unwrap();
        }
        let nested = Expr::timing(
            TimingOperator::Add,
            vec![
                Expr::timing(
                    TimingOperator::Elmore,
                    vec![
                        Expr::timing(TimingOperator::Wire, vec![Expr::atom("L_y")]),
                        Expr::timing(TimingOperator::Pmos, vec![Expr::atom("5")]),
                    ],
                ),
                Expr::atom("extra_delay"),
            ],
        );
        assignment(Expr::atom("a"), nested).validate().unwrap();
    }

    #[test]
    fn cell_validation_accepts_nested_delays_but_rejects_nested_values() {
        let nested_delay = Expr::timing(
            TimingOperator::Mux,
            vec![
                Expr::timing(
                    TimingOperator::Greater,
                    vec![Expr::atom("rise"), Expr::atom("minimum")],
                ),
                Expr::timing(
                    TimingOperator::Add,
                    vec![Expr::atom("rise"), Expr::atom("extra")],
                ),
                Expr::atom("minimum"),
            ],
        );
        let mut cell = Cell {
            name: "sample".to_string(),
            inputs: vec!["a".to_string(), "b".to_string()],
            outputs: vec!["y".to_string()],
            registers: Vec::new(),
            items: vec![CellItem::Assignment(assignment(
                Expr::value(ValueOperator::And, vec![Expr::atom("a"), Expr::atom("b")]),
                nested_delay,
            ))],
        };
        cell.validate().unwrap();

        cell.items = vec![CellItem::Assignment(assignment(
            Expr::value(
                ValueOperator::And,
                vec![
                    Expr::atom("a"),
                    Expr::value(ValueOperator::Not, vec![Expr::atom("b")]),
                ],
            ),
            Expr::atom("0"),
        ))];
        assert!(cell.validate().is_err());
    }

    #[test]
    fn timing_validation_rejects_unknown_and_wrong_arity_operators() {
        assert!(
            Expr::list(vec![Expr::atom("unknown"), Expr::atom("1")])
                .validate_timing("test")
                .is_err()
        );
        assert!(
            Expr::timing(TimingOperator::Elmore, vec![Expr::atom("1")])
                .validate_timing("test")
                .is_err()
        );
    }

    #[test]
    fn high_z_is_not_an_ordinary_driven_value() {
        assert!(Expr::atom("z").validate_value("test").is_err());
        assert!(
            Expr::value(
                ValueOperator::Mux,
                vec![Expr::atom("select"), Expr::atom("a"), Expr::atom("z")],
            )
            .validate_value("test")
            .is_err()
        );
        Expr::value(
            ValueOperator::CaseEq,
            vec![Expr::atom("a"), Expr::atom("z")],
        )
        .validate_value("test")
        .unwrap();
    }

    #[test]
    fn cell_validation_rejects_empty_names_and_invalid_assignments() {
        let mut cell = Cell {
            name: "sample".to_string(),
            inputs: vec![String::new()],
            outputs: vec!["y".to_string()],
            registers: Vec::new(),
            items: Vec::new(),
        };
        assert_eq!(cell.validate().unwrap_err().context, "inputs");

        cell.inputs = vec!["a".to_string()];
        cell.items.push(CellItem::Assignment(assignment(
            Expr::value(ValueOperator::Not, Vec::new()),
            Expr::atom("0"),
        )));
        assert_eq!(cell.validate().unwrap_err().context, "assignment `y` value");
    }
}
