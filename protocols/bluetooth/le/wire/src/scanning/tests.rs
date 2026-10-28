use super::*;
const ADV_NONCONN: [u8; 11] = [2, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];
#[test]
fn passive_report_retains_protocol_fields_and_metadata() {
    let report = parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel37, -71)
        .expect("the legacy PDU is valid");
    assert_eq!(
        report.kind(),
        LegacyAdvertisingReportKind::NonconnectableUndirected
    );
    assert_eq!(report.advertiser().wire_bytes(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(report.advertiser().kind(), LeDeviceAddressKind::Public);
    assert_eq!(report.data(), [2, 1, 6]);
    assert_eq!(report.channel(), PrimaryScanChannel::Channel37);
    assert_eq!(report.rssi_dbm(), -71);
}

#[test]
fn every_legacy_report_prefix_retains_its_exact_data() {
    for length in 0..=super::LEGACY_SCAN_DATA_CAPACITY {
        let mut pdu = [0x5a; 39];
        pdu[0] = 2;
        pdu[1] = (6 + length) as u8;
        let report =
            parse_legacy_advertising_report(&pdu[..8 + length], PrimaryScanChannel::Channel37, -50)
                .unwrap();
        assert_eq!(report.data(), &pdu[8..8 + length]);
    }
}
