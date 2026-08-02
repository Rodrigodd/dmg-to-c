#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    site: PlacementSite,
    delay: PlacementDelay,
    effects: Vec<CandidateEffect>,
    preference: CandidatePreference,
}

/// A durable physical location at which a delay can be represented.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlacementSite {
    /// The result of an existing emitted assignment, before its target signal.
    ExistingAssignment {
        node: TimingNodeId,
        assignment_order: usize,
    },
    /// One exact dependency occurrence in source/build order.
    DependencyEdge {
        dependency_order: usize,
        source: TimingNodeId,
        target: TimingNodeId,
    },
    /// A typed raw/public split after an output's internal value region.
    PublicOutputSplit { signal: TimingNodeId },
}

/// The actual local delay value carried by one physical placement.
///
/// Empty component vectors mean that the placement contributes no term to
/// that transition. They remain distinct from a vector containing the literal
/// timing term `0`; Phase 3 is responsible for IR-boundary materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementDelay {
    One(Vec<DelayTerm>),
    Two {
        rise: Vec<DelayTerm>,
        fall: Vec<DelayTerm>,
    },
    Three {
        rise: Vec<DelayTerm>,
        fall: Vec<DelayTerm>,
        turn_off: Vec<DelayTerm>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateEffect {
    path_index: usize,
    contribution: AdditiveDelayTupleContribution,
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
    pub fn positions(&self) -> &[usize] {
        &self.positions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CandidatePreference {
    kind: u8,
    non_shared: bool,
    target_distance: usize,
    constraint_order: u32,
    control_order: u32,
    path_order: usize,
    site_order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionalPath {
    public: DecompositionPath,
    sites: Vec<PathSite>,
}

/// Deterministic identity of one epoch-bounded functional path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompositionPath {
    id: DecompositionPathId,
    constraint_id: TimingConstraintId,
    control_id: TimingControlId,
    nodes: Vec<TimingNodeId>,
    dependency_orders: Vec<usize>,
    sites: Vec<PlacementSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSite {
    site: PlacementSite,
    orientation_to_target: PathOrientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathOrientation {
    Positive,
    Negative,
    Ambiguous,
}
