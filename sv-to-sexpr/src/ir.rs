use std::collections::BTreeMap;
use std::fmt;

use crate::diagnostic::Diagnostic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub registers: Vec<Register>,
    pub items: Vec<CellItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicValue {
    Zero,
    One,
    X,
    Z,
}

impl LogicValue {
    pub const ALL: [Self; 4] = [Self::Zero, Self::One, Self::X, Self::Z];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::X => "x",
            Self::Z => "z",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    pub name: String,
    pub initial: LogicValue,
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

/// The exact, source-ordered drive-strength pairs represented by the cell DSL.
///
/// Keeping this contract typed prevents a lowered driver from carrying arbitrary
/// strength atoms that happen to satisfy the operator arity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrengthPair {
    Strong1Highz0,
    Highz1Strong0,
    Pull1Highz0,
    Supply1Supply0,
}

impl StrengthPair {
    pub const ALL: [Self; 4] = [
        Self::Strong1Highz0,
        Self::Highz1Strong0,
        Self::Pull1Highz0,
        Self::Supply1Supply0,
    ];

    pub const fn atoms(self) -> (&'static str, &'static str) {
        match self {
            Self::Strong1Highz0 => ("strong1", "highz0"),
            Self::Highz1Strong0 => ("highz1", "strong0"),
            Self::Pull1Highz0 => ("pull1", "highz0"),
            Self::Supply1Supply0 => ("supply1", "supply0"),
        }
    }

    pub fn parse(first: &str, second: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|pair| pair.atoms() == (first, second))
    }
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
        validate_registers(&self.registers)?;
        for item in &self.items {
            if let CellItem::Assignment(assignment) = item {
                assignment.validate()?;
            }
        }
        Ok(())
    }
}

fn validate_registers(registers: &[Register]) -> Result<(), ValidationError> {
    let mut names = std::collections::BTreeSet::new();
    for register in registers {
        if register.name.is_empty() {
            return Err(ValidationError::new(
                "registers",
                "names must be non-empty atoms",
            ));
        }
        if !names.insert(register.name.as_str()) {
            return Err(ValidationError::new(
                "registers",
                format!("duplicate register name `{}`", register.name),
            ));
        }
    }
    Ok(())
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
                if matches!(
                    operator,
                    ValueOperator::DriveStrength
                        | ValueOperator::BufIf0Strength
                        | ValueOperator::BufIf1Strength
                ) {
                    let first = operands[operands.len() - 2]
                        .as_atom()
                        .expect("validated atom");
                    let second = operands[operands.len() - 1]
                        .as_atom()
                        .expect("validated atom");
                    if StrengthPair::parse(first, second).is_none() {
                        return Err(ValidationError::new(
                            context,
                            format!("unsupported drive strength pair `({first}, {second})`"),
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn as_atom(&self) -> Option<&str> {
        match self {
            Self::Atom(atom) => Some(atom),
            Self::List(_) => None,
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
            let mut operands = (0..arity)
                .map(|index| Expr::atom(format!("a{index}")))
                .collect::<Vec<_>>();
            if matches!(
                operator,
                ValueOperator::DriveStrength
                    | ValueOperator::BufIf0Strength
                    | ValueOperator::BufIf1Strength
            ) {
                let (first, second) = StrengthPair::Strong1Highz0.atoms();
                operands[arity - 2] = Expr::atom(first);
                operands[arity - 1] = Expr::atom(second);
            }
            Expr::value(operator, operands)
                .validate_value("test")
                .unwrap();
        }
    }

    #[test]
    fn strength_operators_accept_only_exact_source_ordered_pairs() {
        for operator in [
            ValueOperator::DriveStrength,
            ValueOperator::BufIf0Strength,
            ValueOperator::BufIf1Strength,
        ] {
            for pair in StrengthPair::ALL {
                let (first, second) = pair.atoms();
                let mut operands = match operator {
                    ValueOperator::DriveStrength => vec![Expr::atom("value")],
                    ValueOperator::BufIf0Strength | ValueOperator::BufIf1Strength => {
                        vec![Expr::atom("value"), Expr::atom("control")]
                    }
                    _ => unreachable!(),
                };
                operands.extend([Expr::atom(first), Expr::atom(second)]);
                Expr::value(operator, operands)
                    .validate_value("test")
                    .unwrap();
            }

            for (first, second) in [
                ("highz0", "strong1"),
                ("strong1", "strong0"),
                ("weak1", "highz0"),
            ] {
                let mut operands = match operator {
                    ValueOperator::DriveStrength => vec![Expr::atom("value")],
                    ValueOperator::BufIf0Strength | ValueOperator::BufIf1Strength => {
                        vec![Expr::atom("value"), Expr::atom("control")]
                    }
                    _ => unreachable!(),
                };
                operands.extend([Expr::atom(first), Expr::atom(second)]);
                let error = Expr::value(operator, operands)
                    .validate_value("test")
                    .unwrap_err();
                assert_eq!(
                    error.message,
                    format!("unsupported drive strength pair `({first}, {second})`")
                );
            }
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

    #[test]
    fn logic_values_have_stable_target_atoms() {
        assert_eq!(
            LogicValue::ALL.map(LogicValue::as_str),
            ["0", "1", "x", "z"]
        );
    }

    #[test]
    fn register_validation_preserves_order_and_rejects_invalid_names() {
        let mut cell = Cell {
            name: "sample".into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            registers: vec![
                Register {
                    name: "first".into(),
                    initial: LogicValue::One,
                },
                Register {
                    name: "second".into(),
                    initial: LogicValue::X,
                },
            ],
            items: Vec::new(),
        };
        cell.validate().unwrap();
        assert_eq!(cell.registers[0].name, "first");
        assert_eq!(cell.registers[1].name, "second");

        cell.registers[1].name.clear();
        let empty = cell.validate().unwrap_err();
        assert_eq!(empty.context, "registers");
        assert_eq!(empty.message, "names must be non-empty atoms");

        cell.registers[1].name = "first".into();
        let duplicate = cell.validate().unwrap_err();
        assert_eq!(duplicate.context, "registers");
        assert_eq!(duplicate.message, "duplicate register name `first`");

        cell.registers.clear();
        cell.validate().unwrap();
    }
}
