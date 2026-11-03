//! One catalog for the host and physical conformance fixtures.

use super::{
    BarrierCase, Execution, Scenario, arena, edca, group_keys, interface_commands, msdu_age,
    outcomes,
};

/// An optional operation required by a case, independently of its air stimulus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Extension {
    /// Data transmission on the two arenas of `TxArena`.
    TxArena,
}

/// One case from the shared suites.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Single-MPDU air outcomes.
    Outcome(outcomes::Case),
    /// Admission and returned-owner lifetime.
    Ownership(Scenario),
    /// Publication versus gate and key barriers.
    Barrier(BarrierCase),
    /// Configuration context FIFO and reuse.
    Configure(interface_commands::Case),
    /// Group-key command custody and progress.
    Group(group_keys::ProgressCase),
    /// Per-VIF parameter selection.
    Edca(edca::Case),
    /// Injected receive-cut boundaries.
    GroupBoundary(group_keys::Case),
    /// Owner-controlled virtual age boundaries.
    Age(msdu_age::Case),
    /// Intake, publication and settlement on the two TX arenas.
    Arena(arena::Case),
}

impl Case {
    /// Stable suite order, with the elementary air outcomes first.
    pub fn all() -> impl Iterator<Item = Self> {
        outcomes::Case::ALL
            .into_iter()
            .map(Self::Outcome)
            .chain(Scenario::ALL.into_iter().map(Self::Ownership))
            .chain(BarrierCase::ALL.into_iter().map(Self::Barrier))
            .chain(interface_commands::Case::all().map(Self::Configure))
            .chain(
                group_keys::ProgressCase::all()
                    .filter(|case| *case != group_keys::ProgressCase::ReplacementRollback)
                    .map(Self::Group),
            )
            .chain(edca::Case::ALL.into_iter().map(Self::Edca))
            .chain(group_keys::Case::ALL.into_iter().map(Self::GroupBoundary))
            .chain(msdu_age::Case::ALL.into_iter().map(Self::Age))
            .chain(arena::Case::ALL.into_iter().map(Self::Arena))
            // The fail-stop rollback stays last: no later case needs the
            // quarantined radio.
            .chain([Self::Group(group_keys::ProgressCase::ReplacementRollback)])
    }

    /// Air or fault stimulus required by this case.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Outcome(case) => case.execution(),
            Self::Ownership(case) => case.execution(),
            Self::Barrier(case) => case.execution(),
            Self::Configure(case) => case.execution(),
            Self::Group(case) => case.execution(),
            Self::Edca(case) => case.execution(),
            Self::GroupBoundary(case) => case.execution(),
            Self::Age(case) => case.execution(),
            Self::Arena(case) => case.execution(),
        }
    }

    /// Requirements follow the operation's form, never its display name.
    pub const fn extension(self) -> Option<Extension> {
        match self {
            Self::Arena(_) => Some(Extension::TxArena),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_stop_rollback_is_last_so_no_later_case_needs_the_quarantined_radio() {
        assert_eq!(
            Case::all().last(),
            Some(Case::Group(group_keys::ProgressCase::ReplacementRollback))
        );
    }

    #[test]
    fn only_arena_cases_need_the_arena_extension() {
        for case in Case::all() {
            assert_eq!(
                case.extension() == Some(Extension::TxArena),
                matches!(case, Case::Arena(_))
            );
        }
    }
}
