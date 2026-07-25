//! Exact structural timing-term handling.
//!
//! This module deliberately implements only one algebraic rule: a timing `+`
//! at the current expression level is associative and may be flattened. Every
//! other expression is an opaque term, including an expression which contains
//! an addition below another operator. Terms are kept in source order and are
//! never simplified, deduplicated, or reordered.

use std::fmt;

use crate::ir::{DelayTuple, Expr, TimingExpr, TimingOperator, ValidationError};

/// An indivisible, structurally validated timing expression.
///
/// "Indivisible" is relative to [`AdditiveDelay`]: only a top-level timing
/// addition is decomposed. The expression retained by a term is otherwise
/// opaque and remains byte-for-byte structural data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayTerm(TimingExpr);

impl DelayTerm {
    pub fn from_timing_expr(expression: TimingExpr) -> Result<Self, TimingTermsError> {
        let term = Self(expression);
        term.validate()?;
        Ok(term)
    }

    pub fn as_timing_expr(&self) -> &TimingExpr {
        &self.0
    }

    pub fn into_timing_expr(self) -> TimingExpr {
        self.0
    }

    pub fn validate(&self) -> Result<(), TimingTermsError> {
        self.0
            .validate("delay term")
            .map_err(TimingTermsError::InvalidTimingExpression)?;
        if is_top_level_add(self.0.as_expr()) {
            return Err(TimingTermsError::TopLevelAdditionIsNotOpaque);
        }
        Ok(())
    }
}

/// A non-empty, source-ordered sequence of opaque additive timing terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveDelay {
    terms: Vec<DelayTerm>,
    source: TimingExpr,
}

impl AdditiveDelay {
    pub fn try_new(terms: Vec<DelayTerm>) -> Result<Self, TimingTermsError> {
        if terms.is_empty() {
            return Err(TimingTermsError::EmptyAdditiveDelay);
        }
        for term in &terms {
            term.validate()?;
        }
        let source = rebuild_terms(&terms)?;
        Ok(Self { terms, source })
    }

    /// Recursively flattens only timing additions visible at the current level.
    pub fn from_timing_expr(expression: TimingExpr) -> Result<Self, TimingTermsError> {
        expression
            .validate("additive delay")
            .map_err(TimingTermsError::InvalidTimingExpression)?;
        let mut terms = Vec::new();
        flatten_addition(expression.as_expr(), &mut terms)?;
        for term in &terms {
            term.validate()?;
        }
        Ok(Self {
            terms,
            source: expression,
        })
    }

    pub fn terms(&self) -> &[DelayTerm] {
        &self.terms
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Reconstructs the exact original addition tree without changing term
    /// structure. Values created directly with [`Self::try_new`] use the
    /// canonical flat addition built from those terms as their source tree.
    pub fn to_timing_expr(&self) -> Result<TimingExpr, TimingTermsError> {
        self.validate()?;
        Ok(self.source.clone())
    }

    pub fn validate(&self) -> Result<(), TimingTermsError> {
        if self.terms.is_empty() {
            return Err(TimingTermsError::EmptyAdditiveDelay);
        }
        for term in &self.terms {
            term.validate()?;
        }
        self.source
            .validate("additive delay source")
            .map_err(TimingTermsError::InvalidTimingExpression)?;
        Ok(())
    }
}

fn rebuild_terms(terms: &[DelayTerm]) -> Result<TimingExpr, TimingTermsError> {
    if terms.len() == 1 {
        return Ok(terms[0].as_timing_expr().clone());
    }
    TimingExpr::operation(
        TimingOperator::Add,
        terms
            .iter()
            .map(|term| term.as_timing_expr().clone())
            .collect(),
    )
    .map_err(TimingTermsError::InvalidTimingExpression)
}

fn flatten_addition(expression: &Expr, terms: &mut Vec<DelayTerm>) -> Result<(), TimingTermsError> {
    if let Expr::List(items) = expression
        && is_top_level_add(expression)
    {
        for operand in &items[1..] {
            flatten_addition(operand, terms)?;
        }
        return Ok(());
    }

    let expression = TimingExpr::try_from_expr(expression.clone())
        .map_err(TimingTermsError::InvalidTimingExpression)?;
    terms.push(DelayTerm::from_timing_expr(expression)?);
    Ok(())
}

fn is_top_level_add(expression: &Expr) -> bool {
    matches!(
        expression,
        Expr::List(items)
            if matches!(
                items.first(),
                Some(Expr::Atom(operator))
                    if TimingOperator::parse(operator) == Some(TimingOperator::Add)
            )
    )
}

/// Tuple-preserving additive views of every independent delay component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdditiveDelayTuple {
    One(AdditiveDelay),
    Two {
        rise: AdditiveDelay,
        fall: AdditiveDelay,
    },
    Three {
        rise: AdditiveDelay,
        fall: AdditiveDelay,
        turn_off: AdditiveDelay,
    },
}

impl AdditiveDelayTuple {
    pub fn from_delay_tuple(tuple: &DelayTuple) -> Result<Self, TimingTermsError> {
        match tuple {
            DelayTuple::One(value) => {
                Ok(Self::One(AdditiveDelay::from_timing_expr(value.clone())?))
            }
            DelayTuple::Two { rise, fall } => Ok(Self::Two {
                rise: AdditiveDelay::from_timing_expr(rise.clone())?,
                fall: AdditiveDelay::from_timing_expr(fall.clone())?,
            }),
            DelayTuple::Three {
                rise,
                fall,
                turn_off,
            } => Ok(Self::Three {
                rise: AdditiveDelay::from_timing_expr(rise.clone())?,
                fall: AdditiveDelay::from_timing_expr(fall.clone())?,
                turn_off: AdditiveDelay::from_timing_expr(turn_off.clone())?,
            }),
        }
    }

    pub const fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Two { .. } => 2,
            Self::Three { .. } => 3,
        }
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn components(&self) -> AdditiveDelayTupleComponents<'_> {
        AdditiveDelayTupleComponents {
            tuple: self,
            index: 0,
        }
    }

    pub fn to_delay_tuple(&self) -> Result<DelayTuple, TimingTermsError> {
        match self {
            Self::One(value) => Ok(DelayTuple::One(value.to_timing_expr()?)),
            Self::Two { rise, fall } => Ok(DelayTuple::Two {
                rise: rise.to_timing_expr()?,
                fall: fall.to_timing_expr()?,
            }),
            Self::Three {
                rise,
                fall,
                turn_off,
            } => Ok(DelayTuple::Three {
                rise: rise.to_timing_expr()?,
                fall: fall.to_timing_expr()?,
                turn_off: turn_off.to_timing_expr()?,
            }),
        }
    }

    fn component(&self, index: usize) -> Option<&AdditiveDelay> {
        match (self, index) {
            (Self::One(value), 0) => Some(value),
            (Self::Two { rise, .. }, 0) | (Self::Three { rise, .. }, 0) => Some(rise),
            (Self::Two { fall, .. }, 1) | (Self::Three { fall, .. }, 1) => Some(fall),
            (Self::Three { turn_off, .. }, 2) => Some(turn_off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdditiveDelayTupleComponents<'a> {
    tuple: &'a AdditiveDelayTuple,
    index: usize,
}

impl<'a> Iterator for AdditiveDelayTupleComponents<'a> {
    type Item = &'a AdditiveDelay;

    fn next(&mut self) -> Option<Self::Item> {
        let component = self.tuple.component(self.index)?;
        self.index += 1;
        Some(component)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.tuple.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for AdditiveDelayTupleComponents<'_> {}
impl std::iter::FusedIterator for AdditiveDelayTupleComponents<'_> {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimingTermsError {
    EmptyAdditiveDelay,
    TopLevelAdditionIsNotOpaque,
    InvalidTimingExpression(ValidationError),
}

impl fmt::Display for TimingTermsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAdditiveDelay => formatter.write_str("additive delay must not be empty"),
            Self::TopLevelAdditionIsNotOpaque => {
                formatter.write_str("a top-level timing addition must be flattened into terms")
            }
            Self::InvalidTimingExpression(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TimingTermsError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(value: &str) -> TimingExpr {
        TimingExpr::atom(value).unwrap()
    }

    fn operation(operator: TimingOperator, operands: Vec<TimingExpr>) -> TimingExpr {
        TimingExpr::operation(operator, operands).unwrap()
    }

    fn add(left: TimingExpr, right: TimingExpr) -> TimingExpr {
        operation(TimingOperator::Add, vec![left, right])
    }

    fn term_expressions(delay: &AdditiveDelay) -> Vec<&Expr> {
        delay
            .terms()
            .iter()
            .map(|term| term.as_timing_expr().as_expr())
            .collect()
    }

    #[test]
    fn recursively_flattens_left_and_right_additions_in_source_order() {
        let source = add(
            add(atom("a"), atom("b")),
            add(atom("c"), add(atom("d"), atom("e"))),
        );
        let flattened = AdditiveDelay::from_timing_expr(source.clone()).unwrap();

        assert_eq!(
            term_expressions(&flattened),
            vec![
                atom("a").as_expr(),
                atom("b").as_expr(),
                atom("c").as_expr(),
                atom("d").as_expr(),
                atom("e").as_expr(),
            ]
        );
        assert_eq!(flattened.to_timing_expr().unwrap(), source);
    }

    #[test]
    fn every_non_add_operator_is_an_opaque_term() {
        for operator in TimingOperator::ALL {
            if operator == TimingOperator::Add {
                continue;
            }
            let operands = match operator {
                TimingOperator::Subtract
                | TimingOperator::Divide
                | TimingOperator::Elmore
                | TimingOperator::Greater => vec![atom("a"), atom("b")],
                TimingOperator::Multiply => vec![atom("a"), atom("b")],
                TimingOperator::Wire | TimingOperator::Pmos | TimingOperator::Nmos => {
                    vec![atom("a")]
                }
                TimingOperator::Mux => vec![atom("a"), atom("b"), atom("c")],
                TimingOperator::Add => unreachable!(),
            };
            let source = operation(operator, operands);
            let flattened = AdditiveDelay::from_timing_expr(source.clone()).unwrap();
            assert_eq!(flattened.len(), 1, "{operator:?}");
            assert_eq!(flattened.to_timing_expr().unwrap(), source, "{operator:?}");
        }
    }

    #[test]
    fn addition_below_an_opaque_operator_remains_nested_and_indivisible() {
        let nested_add = add(atom("a"), atom("b"));
        let source = operation(TimingOperator::Multiply, vec![nested_add, atom("scale")]);
        let flattened = AdditiveDelay::from_timing_expr(source.clone()).unwrap();

        assert_eq!(flattened.len(), 1);
        assert_eq!(flattened.to_timing_expr().unwrap(), source);
    }

    #[test]
    fn flattening_never_simplifies_reorders_or_deduplicates_terms() {
        let opaque_zero_product = operation(
            TimingOperator::Multiply,
            vec![atom("0"), atom("unreachable")],
        );
        let source = add(
            add(atom("same"), atom("0")),
            add(opaque_zero_product.clone(), atom("same")),
        );
        let flattened = AdditiveDelay::from_timing_expr(source).unwrap();

        assert_eq!(
            term_expressions(&flattened),
            vec![
                atom("same").as_expr(),
                atom("0").as_expr(),
                opaque_zero_product.as_expr(),
                atom("same").as_expr(),
            ]
        );
    }

    #[test]
    fn tuple_components_are_flattened_and_rebuilt_independently() {
        let tuple = DelayTuple::Three {
            rise: add(atom("r0"), add(atom("r1"), atom("r2"))),
            fall: operation(
                TimingOperator::Multiply,
                vec![add(atom("f0"), atom("f1")), atom("scale")],
            ),
            turn_off: atom("z"),
        };
        let additive = AdditiveDelayTuple::from_delay_tuple(&tuple).unwrap();

        assert_eq!(additive.len(), 3);
        assert_eq!(
            additive
                .components()
                .map(AdditiveDelay::len)
                .collect::<Vec<_>>(),
            vec![3, 1, 1]
        );
        assert_eq!(additive.to_delay_tuple().unwrap(), tuple);
    }

    #[test]
    fn constructors_and_validation_reject_an_empty_additive_delay() {
        assert_eq!(
            AdditiveDelay::try_new(Vec::new()).unwrap_err(),
            TimingTermsError::EmptyAdditiveDelay
        );

        let term = DelayTerm::from_timing_expr(atom("valid")).unwrap();
        assert!(term.validate().is_ok());
        assert!(
            AdditiveDelay::try_new(vec![term])
                .unwrap()
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn public_term_construction_and_additive_validation_reject_an_opaque_top_level_add() {
        let top_level_add = add(atom("left"), atom("right"));
        assert_eq!(
            DelayTerm::from_timing_expr(top_level_add.clone()).unwrap_err(),
            TimingTermsError::TopLevelAdditionIsNotOpaque
        );

        // `DelayTerm` is private-invariant data. This in-module construction
        // proves `try_new` revalidates it instead of trusting its caller.
        let invalid_term = DelayTerm(top_level_add);
        assert_eq!(
            AdditiveDelay::try_new(vec![invalid_term]).unwrap_err(),
            TimingTermsError::TopLevelAdditionIsNotOpaque
        );

        let opaque_nested_add = operation(
            TimingOperator::Multiply,
            vec![
                add(atom("nested-left"), atom("nested-right")),
                atom("scale"),
            ],
        );
        assert!(DelayTerm::from_timing_expr(opaque_nested_add).is_ok());
    }

    #[test]
    fn tuple_arity_is_preserved_for_one_two_and_three_components() {
        let tuples = [
            DelayTuple::One(atom("one")),
            DelayTuple::Two {
                rise: atom("rise"),
                fall: atom("fall"),
            },
            DelayTuple::Three {
                rise: atom("rise"),
                fall: atom("fall"),
                turn_off: atom("turn-off"),
            },
        ];

        for tuple in tuples {
            let additive = AdditiveDelayTuple::from_delay_tuple(&tuple).unwrap();
            assert_eq!(additive.len(), tuple.len());
            assert_eq!(additive.to_delay_tuple().unwrap(), tuple);
        }
    }
}
