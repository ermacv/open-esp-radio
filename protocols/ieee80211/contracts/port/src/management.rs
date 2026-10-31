//! A management frame's rate and bounded transmission budget.

use core::num::NonZeroU8;

use oer_ieee80211_wire::phy::LegacyRate;

/// A management frame keeps its derived rate for every attempt.
///
/// The caller derives `rate` from the band and any known BSS basic rates
/// and chooses the budget, including the first attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementTxPlan {
    rate: LegacyRate,
    attempts: NonZeroU8,
}

impl ManagementTxPlan {
    /// At most `attempts` transmissions at `rate`.
    pub const fn new(rate: LegacyRate, attempts: NonZeroU8) -> Self {
        Self { rate, attempts }
    }

    /// The derived rate used for every attempt.
    pub const fn rate(self) -> LegacyRate {
        self.rate
    }

    /// Maximum attempts, including the first transmission.
    pub const fn attempts(self) -> u8 {
        self.attempts.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_keeps_its_rate_and_budget() {
        let plan = ManagementTxPlan::new(LegacyRate::Ofdm12M, NonZeroU8::MIN.saturating_add(2));
        assert_eq!(plan.rate(), LegacyRate::Ofdm12M);
        assert_eq!(plan.attempts(), 3);
    }
}
