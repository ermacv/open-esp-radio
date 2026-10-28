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

#[test]
fn scanning_preserves_header_classification_addresses_and_legacy_length_admission() {
    for octet in u8::MIN..=u8::MAX {
        let code = octet % 16;
        let extent = if code == 1 { 14 } else { 8 };
        for length in u8::MIN..=u8::MAX {
            let pdu = [octet, length, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
            let result =
                parse_legacy_advertising_report(&pdu[..extent], PrimaryScanChannel::Channel38, -42);
            let declared = usize::from(length % 64);
            if declared > 37 {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong)
                );
            } else if declared + 2 != extent {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingReportParseError::DeclaredLengthMismatch)
                );
            } else {
                let kind = match code {
                    0 => LegacyAdvertisingReportKind::ConnectableUndirected,
                    1 => LegacyAdvertisingReportKind::ConnectableDirected,
                    2 => LegacyAdvertisingReportKind::NonconnectableUndirected,
                    4 => LegacyAdvertisingReportKind::ScanResponse,
                    6 => LegacyAdvertisingReportKind::ScannableUndirected,
                    other => {
                        assert_eq!(
                            result,
                            Err(LegacyAdvertisingReportParseError::UnsupportedPduType(other))
                        );
                        continue;
                    }
                };
                let report = result.unwrap();
                assert_eq!(report.kind(), kind);
                assert_eq!(
                    report.advertiser(),
                    LeDeviceAddress::from_wire_bytes(
                        [1, 2, 3, 4, 5, 6],
                        if (octet / 64) % 2 == 0 {
                            LeDeviceAddressKind::Public
                        } else {
                            LeDeviceAddressKind::Random
                        }
                    )
                );
                assert_eq!(
                    report.directed_target(),
                    if code == 1 {
                        Some(LeDeviceAddress::from_wire_bytes(
                            [7, 8, 9, 10, 11, 12],
                            if octet < 128 {
                                LeDeviceAddressKind::Public
                            } else {
                                LeDeviceAddressKind::Random
                            },
                        ))
                    } else {
                        None
                    }
                );
                assert!(report.data().is_empty());
            }
        }
    }
}
