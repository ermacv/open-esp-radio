//! One catalog for the host and physical conformance fixtures.

use super::{
    BarrierCase, Execution, Scenario, edca, group_keys, interface_commands, msdu_age, outcomes,
};

/// An optional operation required by a case, independently of its air stimulus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Extension {
    /// Aggregate admission through `AmpduPhase`.
    Ampdu,
}

/// One case from the shared suites.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Single and aggregate air outcomes.
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
            // Append new hardware cases so deployed request ordinals stay stable.
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
        }
    }

    /// Requirements follow the operation's form, never its display name.
    pub const fn extension(self) -> Option<Extension> {
        match self {
            Self::Outcome(case) => match case.form() {
                outcomes::Form::Aggregate => Some(Extension::Ampdu),
                outcomes::Form::Single => None,
            },
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
    fn unsupported_aggregate_operations_keep_their_air_classification() {
        let cases: alloc::vec::Vec<_> = Case::all()
            .filter(|case| {
                case.execution() == Execution::Hardware
                    && case.extension() == Some(Extension::Ampdu)
            })
            .collect();
        assert_eq!(
            cases,
            [
                Case::Outcome(outcomes::Case::BlockAck),
                Case::Outcome(outcomes::Case::NoResponse(outcomes::Form::Aggregate))
            ]
        );
        for case in outcomes::Case::ALL {
            assert_eq!(
                Case::Outcome(case).extension().is_some(),
                case.form() == outcomes::Form::Aggregate
            );
        }
    }
}
