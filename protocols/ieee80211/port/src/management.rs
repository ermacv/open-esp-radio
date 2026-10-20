//! A management frame's rate and bounded transmission budget.

use oer_ieee80211_wire::phy::LegacyRate;

/// A management frame keeps its derived rate for every attempt.
///
/// The caller derives `rate` from the band and any known BSS basic rates.
/// Only the constructors choose the budget, including the first attempt:
/// dot11ShortRetryLimit for unicast, one transmission for broadcast.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementTxPlan {
    rate: LegacyRate,
    attempts: u8,
}

impl ManagementTxPlan {
    /// At most seven attempts, the default dot11ShortRetryLimit
    /// (IEEE Std 802.11-2020, Annex C, dot11ShortRetryLimit).
    pub const fn unicast(rate: LegacyRate) -> Self {
        Self { rate, attempts: 7 }
    }

    /// One transmission; a group-addressed frame solicits no ACK.
    pub const fn broadcast(rate: LegacyRate) -> Self {
        Self { rate, attempts: 1 }
    }

    /// The derived rate used for every attempt.
    pub const fn rate(self) -> LegacyRate {
        self.rate
    }

    /// Maximum attempts, including the first transmission.
    pub const fn attempts(self) -> u8 {
        self.attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicast_keeps_the_rate_with_the_short_retry_budget() {
        let plan = ManagementTxPlan::unicast(LegacyRate::Ofdm12M);
        assert_eq!(plan.rate(), LegacyRate::Ofdm12M);
        assert_eq!(plan.attempts(), 7);
    }

    #[test]
    fn broadcast_gets_one_attempt_at_the_selected_rate() {
        let plan = ManagementTxPlan::broadcast(LegacyRate::Ofdm6M);
        assert_eq!(plan.rate(), LegacyRate::Ofdm6M);
        assert_eq!(plan.attempts(), 1);
    }
}
