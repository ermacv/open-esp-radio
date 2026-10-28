#![no_std]
use {oer_ieee80211_wire::ccmp::CcmpPacketNumber, oer_ieee80211_upper_mac::ccmp::{CcmpReplayCandidate, CcmpRxReplayState}};

pub fn check(
    state: &mut CcmpRxReplayState,
    foreign: &mut CcmpPacketNumber,
    packet_number: CcmpPacketNumber,
) {
    #[cfg(ui_reject)]
    let candidate = CcmpReplayCandidate {
        frontier: foreign, // ui-error: E0451
        packet_number,
    };
    #[cfg(not(ui_reject))]
    let candidate = state.prepare(oer_ieee80211_upper_mac::ccmp::CcmpReplayLane::NonQos, packet_number).unwrap();
    candidate.commit();
}
