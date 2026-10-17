#![no_std]
use oer_ieee80211_mac::ccmp::{
    CcmpPacketNumber, CcmpReplayLane, CcmpRxReplayCandidate, CcmpRxReplayState,
};

pub fn check(
    state: &mut CcmpRxReplayState,
    foreign: &mut CcmpPacketNumber,
    packet_number: CcmpPacketNumber,
) {
    let lane = CcmpReplayLane::NonQos;
    #[cfg(ui_reject)]
    let candidate = CcmpRxReplayCandidate {
        frontier: foreign, // ui-error: E0451
        lane,
        packet_number,
    };
    #[cfg(not(ui_reject))]
    let candidate = state.prepare(lane, packet_number).unwrap();
    candidate.commit();
}
