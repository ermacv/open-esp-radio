//! How long one scenario instance takes.

use crate::{Overflow, Provisional, StandProfile, Wait, WaitError, arithmetic};

/// The derived time of one instance's own work on the stand. The plan adds
/// what every instance costs beyond it (its boot to Hello) from the stand
/// profile, so no scenario can leave it out. [`Estimate::from_waits`] is
/// its only constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Estimate {
    setup_micros: u64,
    repetition_micros: u64,
    repetitions: u64,
    provisional: Vec<Provisional>,
}

impl Estimate {
    /// An estimate whose setup and repetition are the sums of their waits.
    pub fn from_waits(
        setup: &[Wait],
        repetition: &[Wait],
        repetitions: u64,
        profile: &StandProfile,
    ) -> Result<Self, WaitError> {
        let mut provisional = Vec::new();
        let mut sum = |waits: &[Wait]| -> Result<u64, WaitError> {
            let mut total = 0_u64;
            for wait in waits {
                let resolved = wait.micros(profile)?;
                total = arithmetic::add(total, resolved.micros)?;
                provisional.extend(resolved.provisional);
            }
            Ok(total)
        };
        let setup_micros = sum(setup)?;
        let repetition_micros = sum(repetition)?;
        provisional.sort();
        provisional.dedup();
        Ok(Self {
            setup_micros,
            repetition_micros,
            repetitions,
            provisional,
        })
    }

    pub fn total_micros(&self) -> Result<u64, Overflow> {
        arithmetic::add(
            self.setup_micros,
            arithmetic::mul(self.repetition_micros, self.repetitions)?,
        )
    }

    /// The provisional stand spans the waits used.
    pub fn provisional(&self) -> &[Provisional] {
        &self.provisional
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use super::*;
    use crate::{Factor, RawStandProfile, Support, name, profile::tests::span};

    #[allow(
        clippy::disallowed_methods,
        reason = "stands for a port's CEILINGS constant"
    )]
    const fn ceiling(micros: u64) -> oer_time::Duration {
        oer_time::Duration::from_micros(micros)
    }

    #[test]
    fn an_estimate_sums_its_waits_and_collects_provisional_labels() {
        let fixture = name!(SpanName, "fixture.hostapd");
        let mut raw = RawStandProfile::default();
        raw.spans.insert(
            fixture.clone(),
            span(
                1_000_000,
                Support::Provisional {
                    reason: "first runs".into(),
                },
            ),
        );
        let profile = StandProfile::try_from(raw).unwrap();
        let estimate = Estimate::from_waits(
            &[Wait::measured(fixture)],
            &[Wait::contract(
                ceiling(2_000),
                Factor::times(NonZeroU32::new(2).unwrap()),
            )],
            5,
            &profile,
        )
        .unwrap();
        assert_eq!(estimate.total_micros(), Ok(1_200_000 + 5 * 4_000));
        assert_eq!(estimate.provisional().len(), 1);
    }

    #[test]
    fn an_overflowing_total_is_an_error() {
        let wait = Wait::contract(ceiling(u64::MAX), Factor::ONE);
        let estimate = Estimate::from_waits(&[], &[wait], 2, &StandProfile::empty()).unwrap();
        assert_eq!(estimate.total_micros(), Err(Overflow));
    }
}
