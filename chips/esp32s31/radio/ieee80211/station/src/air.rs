//! The S31's charge of a frame whose cost is not known: the backend's
//! ceiling of its cost model ([`msdu_cost_ceiling_micros`]) over the bounds
//! this composition executes, for the station's airtime accounts
//! ([`runtime::StationAir`](oer_protocols_ieee80211_runtime::station::StationAir)).

use core::cell::Cell;
use core::num::NonZeroU32;

use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::arena::{
    SLOWEST_SINGLE, msdu_cost_ceiling_micros,
};
use oer_chips_family_radio_ieee80211_policy::rate_schedule::MAX_SCHEDULE_PUBLICATION_LIMIT;
use oer_protocols_ieee80211_runtime::station::FrameCost;
use oer_protocols_ieee80211_wire::phy::PhyRate;

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

/// The S31 station's frame costs: the least useful grant at the full rate
/// the association last went at, or the slowest single rate the data plans
/// publish until one reported, and the ceiling over the data plane's
/// aggregate attempts, then the largest single plan any rate record walks.
///
/// Each answer is kept with every input it was computed from: the least
/// grant with the full rate (mode, MCS, streams, width, guard interval and
/// LTF) and the frame length, the ceiling with the length, whose bounds are
/// this composition's constants. Another input computes it again.
#[derive(Debug, Default)]
pub struct S31Ceiling {
    least: Cell<Option<(Option<PhyRate>, usize, NonZeroU32)>>,
    ceiling: Cell<Option<(usize, NonZeroU32)>>,
}

impl S31Ceiling {
    pub const fn new() -> Self {
        Self {
            least: Cell::new(None),
            ceiling: Cell::new(None),
        }
    }
}

/// The least grant of `frame_octets` at `rate`, or at the slowest single
/// rate without one; a frame no PPDU at that rate can carry asks the least
/// nonzero grant.
fn least_at(frame_octets: usize, rate: Option<PhyRate>) -> NonZeroU32 {
    let octets = u32::try_from(frame_octets)
        .unwrap_or(u32::MAX)
        .saturating_add(MPDU_GROWTH);
    rate.unwrap_or(SLOWEST_SINGLE)
        .max_ppdu_duration_micros(octets)
        .and_then(NonZeroU32::new)
        .unwrap_or(NonZeroU32::MIN)
}

/// The ceiling of `frame_octets` over the executed bounds; one that does
/// not fit a `u32` charges the most a `u32` holds.
fn ceiling_of(frame_octets: usize) -> NonZeroU32 {
    msdu_cost_ceiling_micros(
        frame_octets,
        AGGREGATE_MEMBER_ATTEMPTS,
        MAX_SCHEDULE_PUBLICATION_LIMIT,
    )
    .unwrap_or(NonZeroU32::MAX)
}

impl FrameCost for S31Ceiling {
    fn least(&self, frame_octets: usize, rate: Option<PhyRate>) -> NonZeroU32 {
        match self.least.get() {
            Some((at, octets, micros)) if at == rate && octets == frame_octets => micros,
            _ => {
                let micros = least_at(frame_octets, rate);
                self.least.set(Some((rate, frame_octets, micros)));
                micros
            }
        }
    }

    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        match self.ceiling.get() {
            Some((octets, micros)) if octets == frame_octets => micros,
            _ => {
                let micros = ceiling_of(frame_octets);
                self.ceiling.set(Some((frame_octets, micros)));
                micros
            }
        }
    }
}

#[cfg(test)]
mod tests;
