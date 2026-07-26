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

    pub fn structurally_eq(&self, other: &Self) -> bool {
        self == other
    }
}

/// A non-empty, source-ordered sequence of opaque additive timing terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveDelay {
    terms: Vec<DelayTerm>,
    source: TimingExpr,
}

/// A checked half-open range of source term positions.
///
/// Empty ranges are valid and select an absent contribution. Bounds are
/// checked against a particular [`AdditiveDelay`] by
/// [`AdditiveDelay::select_range`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermRange {
    start: usize,
    end: usize,
}

impl TermRange {
    pub fn new(start: usize, end: usize) -> Result<Self, TimingTermsError> {
        if start > end {
            return Err(TimingTermsError::ReversedTermRange { start, end });
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// An exact selection of whole terms from one additive delay component.
///
/// Positions are retained alongside the structural terms so duplicate equal
/// terms remain unambiguous. An empty selection represents no contribution;
/// it is intentionally distinct from selecting a source term whose expression
/// is the literal atom `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveDelayContribution {
    source_len: usize,
    positions: Vec<usize>,
    terms: Vec<DelayTerm>,
}

impl AdditiveDelayContribution {
    pub const fn source_len(&self) -> usize {
        self.source_len
    }

    pub fn positions(&self) -> &[usize] {
        &self.positions
    }

    pub fn terms(&self) -> &[DelayTerm] {
        &self.terms
    }

    pub const fn len(&self) -> usize {
        self.terms.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Builds a canonical expression for a non-empty contribution.
    ///
    /// This is not source-tree recovery: a contribution can be only part of a
    /// source expression. Exact recovery is provided by
    /// [`AdditiveDelay::recompose_contributions`].
    pub fn to_canonical_timing_expr(&self) -> Result<Option<TimingExpr>, TimingTermsError> {
        if self.terms.is_empty() {
            return Ok(None);
        }
        rebuild_terms(&self.terms).map(Some)
    }
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

    /// Finds every overlapping occurrence of a non-empty structural term
    /// sequence. Results are ordered by their source start position.
    pub fn matching_ranges(
        &self,
        needle: &[DelayTerm],
    ) -> Result<Vec<TermRange>, TimingTermsError> {
        if needle.is_empty() {
            return Err(TimingTermsError::EmptyMatchPattern);
        }
        if needle.len() > self.terms.len() {
            return Ok(Vec::new());
        }

        Ok(self
            .terms
            .windows(needle.len())
            .enumerate()
            .filter_map(|(start, window)| {
                (window == needle).then_some(TermRange {
                    start,
                    end: start + needle.len(),
                })
            })
            .collect())
    }

    pub fn contains_terms(&self, needle: &[DelayTerm]) -> Result<bool, TimingTermsError> {
        Ok(!self.matching_ranges(needle)?.is_empty())
    }

    /// Selects whole terms by strictly increasing source positions.
    pub fn select_positions(
        &self,
        positions: &[usize],
    ) -> Result<AdditiveDelayContribution, TimingTermsError> {
        let mut previous = None;
        let mut terms = Vec::with_capacity(positions.len());
        for &position in positions {
            if position >= self.terms.len() {
                return Err(TimingTermsError::TermPositionOutOfBounds {
                    position,
                    source_len: self.terms.len(),
                });
            }
            if let Some(previous) = previous
                && position <= previous
            {
                return Err(TimingTermsError::TermPositionsNotStrictlyIncreasing {
                    previous,
                    current: position,
                });
            }
            terms.push(self.terms[position].clone());
            previous = Some(position);
        }
        Ok(AdditiveDelayContribution {
            source_len: self.terms.len(),
            positions: positions.to_vec(),
            terms,
        })
    }

    pub fn select_range(
        &self,
        range: TermRange,
    ) -> Result<AdditiveDelayContribution, TimingTermsError> {
        if range.end > self.terms.len() {
            return Err(TimingTermsError::TermRangeOutOfBounds {
                start: range.start,
                end: range.end,
                source_len: self.terms.len(),
            });
        }
        self.select_positions(&(range.start..range.end).collect::<Vec<_>>())
    }

    /// Recovers the retained source tree only when the supplied contributions
    /// cover every source term exactly once and in source order.
    pub fn recompose_contributions(
        &self,
        contributions: &[AdditiveDelayContribution],
    ) -> Result<TimingExpr, TimingTermsError> {
        self.validate()?;
        let mut ordered_positions = Vec::new();
        let mut seen = vec![false; self.terms.len()];

        for contribution in contributions {
            if contribution.source_len != self.terms.len() {
                return Err(TimingTermsError::ContributionSourceLengthMismatch {
                    expected: self.terms.len(),
                    actual: contribution.source_len,
                });
            }
            if contribution.positions.len() != contribution.terms.len() {
                return Err(TimingTermsError::ContributionPositionTermCountMismatch {
                    positions: contribution.positions.len(),
                    terms: contribution.terms.len(),
                });
            }
            for (&position, term) in contribution.positions.iter().zip(&contribution.terms) {
                if position >= self.terms.len() {
                    return Err(TimingTermsError::TermPositionOutOfBounds {
                        position,
                        source_len: self.terms.len(),
                    });
                }
                if self.terms[position] != *term {
                    return Err(TimingTermsError::ContributionTermMismatch { position });
                }
                if seen[position] {
                    return Err(TimingTermsError::ContributionOverlap { position });
                }
                seen[position] = true;
                ordered_positions.push(position);
            }
        }

        if let Some(position) = seen.iter().position(|seen| !seen) {
            return Err(TimingTermsError::ContributionGap { position });
        }
        if let Some((order, &position)) = ordered_positions
            .iter()
            .enumerate()
            .find(|(order, position)| **position != *order)
        {
            return Err(TimingTermsError::ContributionsReordered { order, position });
        }
        Ok(self.source.clone())
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

    pub fn structurally_eq(&self, other: &Self) -> bool {
        self == other
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

/// Tuple-arity-preserving selections for one delay placement.
///
/// Every component is present as a selection, but the selection may be empty.
/// This preserves the difference between an absent contribution and a literal
/// `0` term until assignment IR materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdditiveDelayTupleContribution {
    One(AdditiveDelayContribution),
    Two {
        rise: AdditiveDelayContribution,
        fall: AdditiveDelayContribution,
    },
    Three {
        rise: AdditiveDelayContribution,
        fall: AdditiveDelayContribution,
        turn_off: AdditiveDelayContribution,
    },
}

impl AdditiveDelayTupleContribution {
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

    pub fn component(&self, index: usize) -> Option<&AdditiveDelayContribution> {
        match (self, index) {
            (Self::One(value), 0) => Some(value),
            (Self::Two { rise, .. }, 0) | (Self::Three { rise, .. }, 0) => Some(rise),
            (Self::Two { fall, .. }, 1) | (Self::Three { fall, .. }, 1) => Some(fall),
            (Self::Three { turn_off, .. }, 2) => Some(turn_off),
            _ => None,
        }
    }

    pub fn components(&self) -> AdditiveDelayTupleContributionComponents<'_> {
        AdditiveDelayTupleContributionComponents {
            tuple: self,
            index: 0,
        }
    }

    /// Applies negative-unate transition sense to a placement contribution.
    /// A one-entry tuple applies to every transition and is unchanged.
    pub fn swapped_rise_fall(&self) -> Self {
        match self {
            Self::One(value) => Self::One(value.clone()),
            Self::Two { rise, fall } => Self::Two {
                rise: fall.clone(),
                fall: rise.clone(),
            },
            Self::Three {
                rise,
                fall,
                turn_off,
            } => Self::Three {
                rise: fall.clone(),
                fall: rise.clone(),
                turn_off: turn_off.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdditiveDelayTupleContributionComponents<'a> {
    tuple: &'a AdditiveDelayTupleContribution,
    index: usize,
}

impl<'a> Iterator for AdditiveDelayTupleContributionComponents<'a> {
    type Item = &'a AdditiveDelayContribution;

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

impl ExactSizeIterator for AdditiveDelayTupleContributionComponents<'_> {}
impl std::iter::FusedIterator for AdditiveDelayTupleContributionComponents<'_> {}

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

    pub fn component(&self, index: usize) -> Option<&AdditiveDelay> {
        match (self, index) {
            (Self::One(value), 0) => Some(value),
            (Self::Two { rise, .. }, 0) | (Self::Three { rise, .. }, 0) => Some(rise),
            (Self::Two { fall, .. }, 1) | (Self::Three { fall, .. }, 1) => Some(fall),
            (Self::Three { turn_off, .. }, 2) => Some(turn_off),
            _ => None,
        }
    }

    pub fn same_arity(&self, other: &Self) -> bool {
        self.len() == other.len()
    }

    pub fn structurally_eq(&self, other: &Self) -> bool {
        self == other
    }

    pub fn select_positions(
        &self,
        positions: &[Vec<usize>],
    ) -> Result<AdditiveDelayTupleContribution, TimingTermsError> {
        if positions.len() != self.len() {
            return Err(TimingTermsError::TupleArityMismatch {
                expected: self.len(),
                actual: positions.len(),
            });
        }
        match self {
            Self::One(value) => Ok(AdditiveDelayTupleContribution::One(
                value
                    .select_positions(&positions[0])
                    .map_err(|error| error.at_tuple_component(0))?,
            )),
            Self::Two { rise, fall } => Ok(AdditiveDelayTupleContribution::Two {
                rise: rise
                    .select_positions(&positions[0])
                    .map_err(|error| error.at_tuple_component(0))?,
                fall: fall
                    .select_positions(&positions[1])
                    .map_err(|error| error.at_tuple_component(1))?,
            }),
            Self::Three {
                rise,
                fall,
                turn_off,
            } => Ok(AdditiveDelayTupleContribution::Three {
                rise: rise
                    .select_positions(&positions[0])
                    .map_err(|error| error.at_tuple_component(0))?,
                fall: fall
                    .select_positions(&positions[1])
                    .map_err(|error| error.at_tuple_component(1))?,
                turn_off: turn_off
                    .select_positions(&positions[2])
                    .map_err(|error| error.at_tuple_component(2))?,
            }),
        }
    }

    pub fn select_ranges(
        &self,
        ranges: &[TermRange],
    ) -> Result<AdditiveDelayTupleContribution, TimingTermsError> {
        if ranges.len() != self.len() {
            return Err(TimingTermsError::TupleArityMismatch {
                expected: self.len(),
                actual: ranges.len(),
            });
        }
        match self {
            Self::One(value) => Ok(AdditiveDelayTupleContribution::One(
                value
                    .select_range(ranges[0])
                    .map_err(|error| error.at_tuple_component(0))?,
            )),
            Self::Two { rise, fall } => Ok(AdditiveDelayTupleContribution::Two {
                rise: rise
                    .select_range(ranges[0])
                    .map_err(|error| error.at_tuple_component(0))?,
                fall: fall
                    .select_range(ranges[1])
                    .map_err(|error| error.at_tuple_component(1))?,
            }),
            Self::Three {
                rise,
                fall,
                turn_off,
            } => Ok(AdditiveDelayTupleContribution::Three {
                rise: rise
                    .select_range(ranges[0])
                    .map_err(|error| error.at_tuple_component(0))?,
                fall: fall
                    .select_range(ranges[1])
                    .map_err(|error| error.at_tuple_component(1))?,
                turn_off: turn_off
                    .select_range(ranges[2])
                    .map_err(|error| error.at_tuple_component(2))?,
            }),
        }
    }

    pub fn empty_contribution(&self) -> AdditiveDelayTupleContribution {
        self.select_positions(&vec![Vec::new(); self.len()])
            .expect("empty in-range selections preserve tuple arity")
    }

    pub fn recompose_contributions(
        &self,
        contributions: &[AdditiveDelayTupleContribution],
    ) -> Result<DelayTuple, TimingTermsError> {
        for contribution in contributions {
            if contribution.len() != self.len() {
                return Err(TimingTermsError::TupleArityMismatch {
                    expected: self.len(),
                    actual: contribution.len(),
                });
            }
        }

        let recompose_component = |index| {
            let component = self
                .component(index)
                .expect("component index is bounded by tuple arity");
            let selected = contributions
                .iter()
                .map(|contribution| {
                    contribution
                        .component(index)
                        .expect("contribution arity was checked")
                        .clone()
                })
                .collect::<Vec<_>>();
            component
                .recompose_contributions(&selected)
                .map_err(|error| error.at_tuple_component(index))
        };

        match self {
            Self::One(_) => Ok(DelayTuple::One(recompose_component(0)?)),
            Self::Two { .. } => Ok(DelayTuple::Two {
                rise: recompose_component(0)?,
                fall: recompose_component(1)?,
            }),
            Self::Three { .. } => Ok(DelayTuple::Three {
                rise: recompose_component(0)?,
                fall: recompose_component(1)?,
                turn_off: recompose_component(2)?,
            }),
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
    EmptyMatchPattern,
    ReversedTermRange {
        start: usize,
        end: usize,
    },
    TermRangeOutOfBounds {
        start: usize,
        end: usize,
        source_len: usize,
    },
    TermPositionOutOfBounds {
        position: usize,
        source_len: usize,
    },
    TermPositionsNotStrictlyIncreasing {
        previous: usize,
        current: usize,
    },
    ContributionSourceLengthMismatch {
        expected: usize,
        actual: usize,
    },
    ContributionPositionTermCountMismatch {
        positions: usize,
        terms: usize,
    },
    ContributionTermMismatch {
        position: usize,
    },
    ContributionOverlap {
        position: usize,
    },
    ContributionGap {
        position: usize,
    },
    ContributionsReordered {
        order: usize,
        position: usize,
    },
    TupleComponent {
        component: usize,
        error: Box<TimingTermsError>,
    },
    TupleArityMismatch {
        expected: usize,
        actual: usize,
    },
    TopLevelAdditionIsNotOpaque,
    InvalidTimingExpression(ValidationError),
}

impl fmt::Display for TimingTermsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAdditiveDelay => formatter.write_str("additive delay must not be empty"),
            Self::EmptyMatchPattern => {
                formatter.write_str("an ordered timing-term match must not be empty")
            }
            Self::ReversedTermRange { start, end } => {
                write!(formatter, "timing-term range {start}..{end} is reversed")
            }
            Self::TermRangeOutOfBounds {
                start,
                end,
                source_len,
            } => write!(
                formatter,
                "timing-term range {start}..{end} exceeds source length {source_len}"
            ),
            Self::TermPositionOutOfBounds {
                position,
                source_len,
            } => write!(
                formatter,
                "timing-term position {position} exceeds source length {source_len}"
            ),
            Self::TermPositionsNotStrictlyIncreasing { previous, current } => write!(
                formatter,
                "timing-term positions are not strictly increasing: {previous} then {current}"
            ),
            Self::ContributionSourceLengthMismatch { expected, actual } => write!(
                formatter,
                "timing contribution source length mismatch: expected {expected}, got {actual}"
            ),
            Self::ContributionPositionTermCountMismatch { positions, terms } => write!(
                formatter,
                "timing contribution has {positions} positions but {terms} terms"
            ),
            Self::ContributionTermMismatch { position } => write!(
                formatter,
                "timing contribution term at source position {position} does not match"
            ),
            Self::ContributionOverlap { position } => write!(
                formatter,
                "timing contributions overlap at source position {position}"
            ),
            Self::ContributionGap { position } => write!(
                formatter,
                "timing contributions leave a gap at source position {position}"
            ),
            Self::ContributionsReordered { order, position } => write!(
                formatter,
                "timing contributions are reordered: reconstructed term {order} came from source position {position}"
            ),
            Self::TupleComponent { component, error } => {
                write!(formatter, "timing tuple component {component}: {error}")
            }
            Self::TupleArityMismatch { expected, actual } => write!(
                formatter,
                "timing tuple arity mismatch: expected {expected}, got {actual}"
            ),
            Self::TopLevelAdditionIsNotOpaque => {
                formatter.write_str("a top-level timing addition must be flattened into terms")
            }
            Self::InvalidTimingExpression(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TimingTermsError {}

impl TimingTermsError {
    fn at_tuple_component(self, component: usize) -> Self {
        Self::TupleComponent {
            component,
            error: Box::new(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

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

    fn additive_atoms(values: &[u8]) -> AdditiveDelay {
        AdditiveDelay::try_new(
            values
                .iter()
                .map(|value| DelayTerm::from_timing_expr(atom(&format!("v{value}"))).unwrap())
                .collect(),
        )
        .unwrap()
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
        for operator in [TimingOperator::Multiply, TimingOperator::Elmore] {
            let source = operation(operator, vec![nested_add.clone(), atom("scale")]);
            let flattened = AdditiveDelay::from_timing_expr(source.clone()).unwrap();

            assert_eq!(flattened.len(), 1, "{operator:?}");
            assert_eq!(flattened.to_timing_expr().unwrap(), source, "{operator:?}");
        }
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
            let empty = additive.empty_contribution();
            assert_eq!(empty.len(), tuple.len());
            assert!(empty.components().all(AdditiveDelayContribution::is_empty));
            assert_eq!(additive.to_delay_tuple().unwrap(), tuple);
        }
    }

    #[test]
    fn ordered_matching_finds_prefix_middle_suffix_and_absence() {
        let source = AdditiveDelay::from_timing_expr(add(
            add(atom("a"), atom("b")),
            add(atom("c"), atom("d")),
        ))
        .unwrap();
        let terms = source.terms();

        assert_eq!(
            source.matching_ranges(&terms[0..2]).unwrap(),
            vec![TermRange::new(0, 2).unwrap()]
        );
        assert_eq!(
            source.matching_ranges(&terms[1..3]).unwrap(),
            vec![TermRange::new(1, 3).unwrap()]
        );
        assert_eq!(
            source.matching_ranges(&terms[2..4]).unwrap(),
            vec![TermRange::new(2, 4).unwrap()]
        );
        let absent = DelayTerm::from_timing_expr(atom("absent")).unwrap();
        assert!(
            source
                .matching_ranges(std::slice::from_ref(&absent))
                .unwrap()
                .is_empty()
        );
        assert!(!source.contains_terms(&[absent]).unwrap());
        assert_eq!(
            source.matching_ranges(&[]).unwrap_err(),
            TimingTermsError::EmptyMatchPattern
        );
    }

    #[test]
    fn ordered_matching_reports_every_overlapping_duplicate_occurrence() {
        let source = additive_atoms(&[1, 1, 1, 2, 1]);
        let repeated = source.terms()[0..2].to_vec();

        let expected = vec![TermRange::new(0, 2).unwrap(), TermRange::new(1, 3).unwrap()];
        assert_eq!(source.matching_ranges(&repeated).unwrap(), expected);
        assert_eq!(source.matching_ranges(&repeated).unwrap(), expected);
    }

    #[test]
    fn empty_contribution_is_distinct_from_a_literal_zero_term() {
        let source = AdditiveDelay::from_timing_expr(add(atom("0"), atom("tail"))).unwrap();
        let empty = source.select_range(TermRange::new(0, 0).unwrap()).unwrap();
        let literal_zero = source.select_range(TermRange::new(0, 1).unwrap()).unwrap();

        assert!(empty.is_empty());
        assert_eq!(empty.positions(), &[]);
        assert_eq!(empty.to_canonical_timing_expr().unwrap(), None);
        assert!(!literal_zero.is_empty());
        assert_eq!(
            literal_zero.to_canonical_timing_expr().unwrap(),
            Some(atom("0"))
        );
    }

    #[test]
    fn exact_factor_and_recompose_recovers_the_retained_source_tree() {
        let source_expr = add(
            add(atom("a"), atom("b")),
            add(atom("c"), add(atom("d"), atom("e"))),
        );
        let source = AdditiveDelay::from_timing_expr(source_expr.clone()).unwrap();
        let contributions = [
            source.select_range(TermRange::new(0, 2).unwrap()).unwrap(),
            source.select_positions(&[2]).unwrap(),
            source.select_range(TermRange::new(3, 5).unwrap()).unwrap(),
        ];

        assert_eq!(
            source.recompose_contributions(&contributions).unwrap(),
            source_expr
        );
    }

    #[test]
    fn selection_rejects_bad_ranges_and_positions() {
        let source = additive_atoms(&[0, 1, 2]);

        assert_eq!(
            TermRange::new(2, 1).unwrap_err(),
            TimingTermsError::ReversedTermRange { start: 2, end: 1 }
        );
        assert_eq!(
            source
                .select_range(TermRange::new(2, 4).unwrap())
                .unwrap_err(),
            TimingTermsError::TermRangeOutOfBounds {
                start: 2,
                end: 4,
                source_len: 3,
            }
        );
        assert_eq!(
            source.select_positions(&[0, 3]).unwrap_err(),
            TimingTermsError::TermPositionOutOfBounds {
                position: 3,
                source_len: 3,
            }
        );
        assert_eq!(
            source.select_positions(&[1, 1]).unwrap_err(),
            TimingTermsError::TermPositionsNotStrictlyIncreasing {
                previous: 1,
                current: 1,
            }
        );
        assert_eq!(
            source.select_positions(&[2, 1]).unwrap_err(),
            TimingTermsError::TermPositionsNotStrictlyIncreasing {
                previous: 2,
                current: 1,
            }
        );
    }

    #[test]
    fn recomposition_reports_overlap_gap_reorder_and_term_mismatch() {
        let source = additive_atoms(&[0, 1, 2]);
        let overlap = [
            source.select_range(TermRange::new(0, 2).unwrap()).unwrap(),
            source.select_range(TermRange::new(1, 3).unwrap()).unwrap(),
        ];
        assert_eq!(
            source.recompose_contributions(&overlap).unwrap_err(),
            TimingTermsError::ContributionOverlap { position: 1 }
        );

        let gap = [
            source.select_positions(&[0]).unwrap(),
            source.select_positions(&[2]).unwrap(),
        ];
        assert_eq!(
            source.recompose_contributions(&gap).unwrap_err(),
            TimingTermsError::ContributionGap { position: 1 }
        );

        let reordered = [
            source.select_positions(&[1]).unwrap(),
            source.select_positions(&[0, 2]).unwrap(),
        ];
        assert_eq!(
            source.recompose_contributions(&reordered).unwrap_err(),
            TimingTermsError::ContributionsReordered {
                order: 0,
                position: 1,
            }
        );

        let mut mismatched = source.select_positions(&[0, 1, 2]).unwrap();
        mismatched.terms[1] = DelayTerm::from_timing_expr(atom("wrong")).unwrap();
        assert_eq!(
            source.recompose_contributions(&[mismatched]).unwrap_err(),
            TimingTermsError::ContributionTermMismatch { position: 1 }
        );
    }

    #[test]
    fn tuple_selection_preserves_arity_empty_components_and_transition_swap() {
        let tuple = DelayTuple::Three {
            rise: add(atom("r0"), atom("r1")),
            fall: add(atom("f0"), atom("f1")),
            turn_off: atom("z"),
        };
        let source = AdditiveDelayTuple::from_delay_tuple(&tuple).unwrap();
        let empty = source.empty_contribution();
        assert_eq!(empty.len(), 3);
        assert!(empty.components().all(AdditiveDelayContribution::is_empty));

        let contribution = source
            .select_positions(&[vec![0], vec![1], vec![0]])
            .unwrap();
        let swapped = contribution.swapped_rise_fall();
        assert_eq!(
            swapped.component(0).unwrap().terms(),
            contribution.component(1).unwrap().terms()
        );
        assert_eq!(
            swapped.component(1).unwrap().terms(),
            contribution.component(0).unwrap().terms()
        );
        assert_eq!(
            swapped.component(2).unwrap(),
            contribution.component(2).unwrap()
        );
    }

    #[test]
    fn tuple_recomposition_is_joint_and_reports_arity_and_component_mismatch() {
        let tuple = DelayTuple::Two {
            rise: add(atom("r0"), atom("r1")),
            fall: add(atom("f0"), atom("f1")),
        };
        let source = AdditiveDelayTuple::from_delay_tuple(&tuple).unwrap();
        let first = source
            .select_ranges(&[TermRange::new(0, 1).unwrap(), TermRange::new(0, 1).unwrap()])
            .unwrap();
        let second = source
            .select_ranges(&[TermRange::new(1, 2).unwrap(), TermRange::new(1, 2).unwrap()])
            .unwrap();
        assert_eq!(
            source
                .recompose_contributions(&[first.clone(), second])
                .unwrap(),
            tuple
        );

        let wrong_arity_source =
            AdditiveDelayTuple::from_delay_tuple(&DelayTuple::One(atom("only"))).unwrap();
        let wrong_arity = wrong_arity_source.empty_contribution();
        assert_eq!(
            source.recompose_contributions(&[wrong_arity]).unwrap_err(),
            TimingTermsError::TupleArityMismatch {
                expected: 2,
                actual: 1,
            }
        );

        let AdditiveDelayTupleContribution::Two { rise, fall } = first else {
            unreachable!()
        };
        let swapped_components = AdditiveDelayTupleContribution::Two {
            rise: fall,
            fall: rise,
        };
        assert_eq!(
            source
                .recompose_contributions(&[swapped_components])
                .unwrap_err(),
            TimingTermsError::TupleComponent {
                component: 0,
                error: Box::new(TimingTermsError::ContributionTermMismatch { position: 0 }),
            }
        );

        assert_eq!(
            source.select_positions(&[vec![0]]).unwrap_err(),
            TimingTermsError::TupleArityMismatch {
                expected: 2,
                actual: 1,
            }
        );
    }

    proptest! {
        #[test]
        fn generated_ordered_partitions_recompose_exactly(
            values in prop::collection::vec(0_u8..4, 1..16),
            cuts in prop::collection::vec(any::<bool>(), 0..16),
        ) {
            let source = additive_atoms(&values);
            let mut contributions = Vec::new();
            let mut start = 0;
            for position in 1..values.len() {
                if cuts.get(position - 1).copied().unwrap_or(false) {
                    contributions.push(
                        source
                            .select_range(TermRange::new(start, position).unwrap())
                            .unwrap(),
                    );
                    start = position;
                }
            }
            contributions.push(
                source
                    .select_range(TermRange::new(start, values.len()).unwrap())
                    .unwrap(),
            );

            prop_assert_eq!(
                source.recompose_contributions(&contributions).unwrap(),
                source.to_timing_expr().unwrap()
            );
        }

        #[test]
        fn generated_duplicate_matching_is_complete_and_deterministic(
            values in prop::collection::vec(0_u8..4, 1..20),
            needle_value in 0_u8..4,
        ) {
            let source = additive_atoms(&values);
            let needle_source = additive_atoms(&[needle_value]);
            let needle = needle_source.terms();
            let expected = values
                .iter()
                .enumerate()
                .filter(|(_, value)| **value == needle_value)
                .map(|(position, _)| TermRange::new(position, position + 1).unwrap())
                .collect::<Vec<_>>();

            let first = source.matching_ranges(needle).unwrap();
            let second = source.matching_ranges(needle).unwrap();
            prop_assert_eq!(&first, &expected);
            prop_assert_eq!(first, second);
        }
    }
}
