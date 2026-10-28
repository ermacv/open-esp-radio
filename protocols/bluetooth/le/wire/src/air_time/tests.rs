#![allow(clippy::disallowed_macros)]

use super::*;
use crate::connection::LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS;

#[test]
fn a_legacy_pdu_length_holds_its_header_and_at_most_37_payload_bytes() {
    assert_eq!(LegacyPduLength::new(0), None);
    assert_eq!(LegacyPduLength::new(1), None);
    assert_eq!(LegacyPduLength::new(2), Some(LegacyPduLength::MIN));
    assert_eq!(LegacyPduLength::new(39), Some(LegacyPduLength::MAX));
    assert_eq!(LegacyPduLength::new(40), None);
    assert_eq!(LegacyPduLength::new(usize::MAX), None);
    assert_eq!(LegacyPduLength::MIN.get(), 2);
    assert_eq!(LegacyPduLength::MAX.get(), 39);
}

#[test]
fn le_1m_packets_take_eight_microseconds_per_octet_around_their_pdu() {
    // Preamble, Access Address, header, ScanA, AdvA and CRC.
    assert_eq!(
        LEGACY_SCAN_REQ_LE_1M_AIRTIME_MICROS,
        (1 + 4 + 2 + 12 + 3) * 8
    );
    // Preamble, Access Address, header, 34-byte payload and CRC.
    assert_eq!(
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS,
        (1 + 4 + 2 + 34 + 3) * 8
    );
    assert_eq!(T_IFS_MICROS, 150);
}
