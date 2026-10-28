//! The transmission budget of a management frame.

use core::num::NonZeroU8;

use oer_ieee80211_port::ManagementTxPlan;
use oer_ieee80211_wire::phy::LegacyRate;

/// The default dot11ShortRetryLimit, attempts including the first.
/// SOURCE: IEEE Std 802.11-2020, Annex C, dot11ShortRetryLimit.
const SHORT_RETRY_LIMIT: NonZeroU8 = const {
    match NonZeroU8::new(7) {
        Some(limit) => limit,
        None => panic!("dot11ShortRetryLimit is nonzero"),
    }
};

/// An individually addressed management frame: at most seven attempts.
pub const fn unicast(rate: LegacyRate) -> ManagementTxPlan {
    ManagementTxPlan::new(rate, SHORT_RETRY_LIMIT)
}

/// A group-addressed management frame solicits no ACK: one transmission.
pub const fn broadcast(rate: LegacyRate) -> ManagementTxPlan {
    ManagementTxPlan::new(rate, NonZeroU8::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicast_keeps_the_rate_with_the_short_retry_budget() {
        let plan = unicast(LegacyRate::Ofdm12M);
        assert_eq!(plan.rate(), LegacyRate::Ofdm12M);
        assert_eq!(plan.attempts(), 7);
    }

    #[test]
    fn broadcast_gets_one_attempt_at_the_selected_rate() {
        let plan = broadcast(LegacyRate::Ofdm6M);
        assert_eq!(plan.rate(), LegacyRate::Ofdm6M);
        assert_eq!(plan.attempts(), 1);
    }
}
