use super::{
    LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReportKind, PrimaryScanChannel,
    parse_legacy_advertising_report,
};
use crate::LeDeviceAddressKind;

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
fn exact_duplicate_filter_ignores_channel_and_rssi_but_not_data() {
    let first = parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel37, -50)
        .expect("the first PDU is valid");
    let duplicate =
        parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel39, -90)
            .expect("the duplicate PDU is valid");
    let changed = [2, 9, 1, 2, 3, 4, 5, 6, 2, 1, 5];
    let changed = parse_legacy_advertising_report(&changed, PrimaryScanChannel::Channel38, -60)
        .expect("the changed PDU is valid");

    let mut filter = LegacyAdvertisingDuplicateFilter::<2>::new();
    assert!(filter.accept(first));
    assert!(!filter.accept(duplicate));
    assert!(filter.accept(changed));
    filter.clear();
    assert!(filter.accept(first));
}
