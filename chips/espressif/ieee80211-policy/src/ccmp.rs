//! The Espressif CCMP transmit packet-number step.

use oer_ieee80211_mac::ccmp::CcmpPacketNumberStep;

/// How far the Espressif `net80211` stack advances a key's transmit packet
/// number per MPDU.
///
/// SOURCE(esp32s31): the reviewed ESP32-S31 CCMP owner (`oer-esp32s31-ieee80211-mac`,
/// `crypto.rs`) records that the pinned `libnet80211.a` implementation
/// advances by three, so a newly installed key emits PN 3 first; the vendor
/// encapsulation is `libnet80211.a[ieee80211_crypto_ccmp.o]::ccmp_encap`
/// (`knowledge/esp32s31/vendor/facts/provenance.toml`). IEEE Std 802.11-2020
/// requires only a PN that increases per key, so the step is Espressif
/// behaviour, not an IEEE rule; the tree does not record why the vendor
/// steps by three. A backend that follows the vendor numbering supplies it
/// to the portable allocator (`oer-ieee80211-mac::ccmp::CcmpTxPacketNumber`).
pub const TX_PACKET_NUMBER_STEP: CcmpPacketNumberStep = match CcmpPacketNumberStep::new(3) {
    Some(step) => step,
    None => panic!("the step is not zero"),
};
