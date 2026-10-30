#![no_std]
use oer_ieee80211_core_security_rsn::bip::BipPacketNumber;
use oer_ieee80211_core_upper_mac::packet_number::PacketNumberFrontier;

pub fn admit(frontier: &mut PacketNumberFrontier<BipPacketNumber>, ipn: BipPacketNumber) {
    let candidate = frontier.prepare(ipn).unwrap();
    #[cfg(ui_reject)]
    let _other = frontier.prepare(ipn); // ui-error: E0499
    candidate.commit();
}
