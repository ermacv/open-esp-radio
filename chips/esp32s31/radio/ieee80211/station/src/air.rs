//! The S31's charge of a frame whose cost is not known: the backend's
//! ceiling of its cost model ([`msdu_cost_ceiling_micros`]) over the bounds
//! this composition executes, for the station's airtime accounts
//! ([`runtime::StationAir`](oer_protocols_ieee80211_runtime::station::StationAir)).

use core::num::NonZeroU32;

use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::arena::msdu_cost_ceiling_micros;
use oer_chips_family_radio_ieee80211_policy::rate_schedule::MAX_SCHEDULE_PUBLICATION_LIMIT;
use oer_protocols_ieee80211_runtime::station::FrameCost;
use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PhyRate, PpduBandwidth};

/// Aggregates on the air a missing member may miss before the data plane
/// drops it, which the station's transmit Block Ack policy passes on.
///
/// SOURCE(esp32s31): `lmac::LONG_RETRY_LIMIT` (`libpp[lmac.o]::lmacInit`).
pub(crate) const AGGREGATE_MEMBER_ATTEMPTS: u8 =
    oer_chips_family_radio_ieee80211_policy::lmac::LONG_RETRY_LIMIT;

/// What a data MPDU adds to its Ethernet frame on the air, as the least
/// useful grant counts it: the QoS Data, CCMP and LLC/SNAP headers, the MIC
/// and the FCS, less the Ethernet header.
///
/// SOURCE: IEEE Std 802.11-2020 9.3.2.1 and 12.5.3.2.
const MPDU_GROWTH: u32 = 26 + 8 + 8 + 8 + 4 - 14;

/// The S31 station's frame costs: the least useful grant at its fastest
/// single-stream rate, and the ceiling over the data plane's aggregate
/// attempts, then the largest single plan any rate record walks.
#[derive(Clone, Copy, Debug, Default)]
pub struct S31Ceiling;

impl FrameCost for S31Ceiling {
    /// The frame's PPDU at HT MCS 7, 20 MHz, short guard interval, the
    /// fastest single-stream rate the station sends at; a frame no PPDU
    /// can carry asks the least nonzero grant.
    fn least(&self, frame_octets: usize) -> NonZeroU32 {
        let octets = u32::try_from(frame_octets)
            .unwrap_or(u32::MAX)
            .saturating_add(MPDU_GROWTH);
        HtRate::new(HtMcs::SINGLE_STREAM_MAX, PpduBandwidth::Mhz20, true)
            .and_then(|rate| PhyRate::Ht(rate).max_ppdu_duration_micros(octets))
            .and_then(NonZeroU32::new)
            .unwrap_or(NonZeroU32::MIN)
    }

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
