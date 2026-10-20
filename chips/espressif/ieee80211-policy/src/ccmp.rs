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

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_upper_mac::ccmp::{CcmpTxPacketNumber, CcmpTxPacketNumberError};
    use oer_ieee80211_wire::ccmp::CcmpKeyId;
    #[test]
    fn tx_packet_number_emits_the_48_bit_maximum_once_then_fails_closed() {
        let mut packet_number = CcmpTxPacketNumber::continuing_after(
            TX_PACKET_NUMBER_STEP,
            oer_ieee80211_wire::ccmp::CcmpPacketNumber::new(
                u64::from(u16::MAX) << 32 | u64::from(u32::MAX - 3),
            )
            .unwrap(),
        );
        let key_id = CcmpKeyId::new(1).unwrap();
        assert_eq!(
            packet_number.next_header(key_id),
            Ok([0xff, 0xff, 0, 0x60, 0xff, 0xff, 0xff, 0xff])
        );
        assert_eq!(
            packet_number.next_header(key_id),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
        assert_eq!(
            packet_number.next_header(key_id),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
    }

    #[test]
    fn independent_installed_keys_start_at_three() {
        let mut pairwise = CcmpTxPacketNumber::new(TX_PACKET_NUMBER_STEP);
        let mut group = CcmpTxPacketNumber::new(TX_PACKET_NUMBER_STEP);
        let header = pairwise.next_header(CcmpKeyId::PAIRWISE).unwrap();
        assert_eq!(header, [3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(
            group.next_header(CcmpKeyId::new(1).unwrap()),
            Ok([3, 0, 0, 0x60, 0, 0, 0, 0])
        );
        assert_eq!(
            pairwise.next_header(CcmpKeyId::PAIRWISE),
            Ok([6, 0, 0, 0x20, 0, 0, 0, 0])
        );
    }
}
