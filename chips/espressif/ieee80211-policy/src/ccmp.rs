//! The Espressif CCMP transmit packet-number step.

use oer_ieee80211_wire::ccmp::CcmpPacketNumberStep;

/// How far the Espressif `net80211` stack advances a key's transmit packet
/// number per MPDU.
///
/// SOURCE(esp32s31): the pinned `libnet80211.a` implementation advances by
/// three; `libnet80211.a[ieee80211_crypto_ccmp.o]::ccmp_encap`
/// (`chips/esp32s31/vendor/facts/provenance.toml`). IEEE Std 802.11-2020
/// requires a PN that increases per key. The step is Espressif behavior;
/// the source does not establish why the vendor steps by three. The core
/// supplies this step to its allocator; the backend receives the resulting
/// PN with the MPDU.
pub const TX_PACKET_NUMBER_STEP: CcmpPacketNumberStep = match CcmpPacketNumberStep::new(3) {
    Some(step) => step,
    None => panic!("the step is not zero"),
};
