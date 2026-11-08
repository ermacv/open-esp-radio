//! The S31's charge of a frame whose cost is not known: the backend's
//! ceiling of its cost model ([`msdu_cost_ceiling_micros`]) over the bounds
//! this composition executes, for the station's airtime accounts
//! ([`runtime::StationAir`](oer_protocols_ieee80211_runtime::station::StationAir)).

use core::num::NonZeroU32;

use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::arena::msdu_cost_ceiling_micros;
use oer_chips_family_radio_ieee80211_policy::rate_schedule::MAX_SCHEDULE_PUBLICATION_LIMIT;
use oer_protocols_ieee80211_runtime::station::CostCeiling;

/// Aggregates on the air a missing member may miss before the data plane
/// drops it, which the station's transmit Block Ack policy passes on.
///
/// SOURCE(esp32s31): `lmac::LONG_RETRY_LIMIT` (`libpp[lmac.o]::lmacInit`).
pub(crate) const AGGREGATE_MEMBER_ATTEMPTS: u8 =
    oer_chips_family_radio_ieee80211_policy::lmac::LONG_RETRY_LIMIT;

/// The S31 station's ceiling: the data plane's aggregate attempts, then the
/// largest single plan any rate record walks.
#[derive(Clone, Copy, Debug, Default)]
pub struct S31Ceiling;

impl CostCeiling for S31Ceiling {
    /// A bound that does not fit a `u32` charges the most a `u32` holds.
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        msdu_cost_ceiling_micros(
            frame_octets,
            AGGREGATE_MEMBER_ATTEMPTS,
            MAX_SCHEDULE_PUBLICATION_LIMIT,
        )
        .unwrap_or(NonZeroU32::MAX)
    }
}

#[cfg(test)]
mod tests;
