#![no_std]
use oer_ieee80211_rsn::bip::BipPacketNumber;
use oer_ieee80211_upper_mac::packet_number::PacketNumberFrontier;
use oer_ieee80211_wire::ccmp::CcmpPacketNumber;

pub fn admit(frontier: &mut PacketNumberFrontier<BipPacketNumber>, ccmp: CcmpPacketNumber) {
    #[cfg(ui_reject)]
    let candidate = frontier.prepare(ccmp).unwrap(); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let candidate = frontier.prepare(BipPacketNumber::from_bytes([1, 0, 0, 0, 0, 0])).unwrap();
    candidate.commit();
    let _: CcmpPacketNumber = ccmp;
}
