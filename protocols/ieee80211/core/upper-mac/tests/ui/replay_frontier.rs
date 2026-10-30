#![no_std]
use {oer_ieee80211_wire::ccmp::CcmpPacketNumber, oer_ieee80211_core_upper_mac::ccmp::{CcmpReplayCandidate, CcmpRxReplayState}};
use oer_ieee80211_core_upper_mac::packet_number::PacketNumberFrontier;

pub fn check(
    state: &mut CcmpRxReplayState,
    foreign: &mut PacketNumberFrontier<CcmpPacketNumber>,
    packet_number: CcmpPacketNumber,
) {
    #[cfg(ui_reject)]
    let candidate = CcmpReplayCandidate {
        candidate: foreign.prepare(packet_number).unwrap(), // ui-error: E0451
    };
    #[cfg(not(ui_reject))]
    let candidate = state.prepare(oer_ieee80211_core_upper_mac::ccmp::CcmpReplayLane::NonQos, packet_number).unwrap();
    candidate.commit();
}
