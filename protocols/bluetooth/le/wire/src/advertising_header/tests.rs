use super::*;

#[test]
fn prefix_preserves_header_class_address_order_and_truncation() {
    for octet in u8::MIN..=u8::MAX {
        let pdu = [octet, 6, 1, 2, 3, 4, 5, 0xc6];
        for length in 0..pdu.len() {
            assert!(LegacyAdvertisingPduPrefix::decode(&pdu[..length]).is_err());
        }
        let prefix = LegacyAdvertisingPduPrefix::decode(&pdu).unwrap();
        let types = [
            AdvertisingPduType::AdvInd,
            AdvertisingPduType::AdvDirectInd,
            AdvertisingPduType::AdvNonconnInd,
            AdvertisingPduType::ScanReq,
            AdvertisingPduType::ScanRsp,
            AdvertisingPduType::ConnectInd,
            AdvertisingPduType::AdvScanInd,
            AdvertisingPduType::AdvExtInd,
        ];
        let code = octet % 16;
        assert_eq!(
            prefix.header.pdu_type(),
            types
                .get(usize::from(code))
                .copied()
                .unwrap_or(AdvertisingPduType::Reserved(code))
        );
        assert_eq!(prefix.advertiser.wire_bytes(), [1, 2, 3, 4, 5, 0xc6]);
        assert_eq!(
            prefix.advertiser.kind(),
            if (octet / 64) % 2 == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            }
        );
    }
}

#[test]
fn prefix_leaves_declared_length_and_trailing_data_for_complete_packet_admission() {
    for length in u8::MIN..=u8::MAX {
        let pdu = [0x42, length, 1, 2, 3, 4, 5, 0xc6, 9];
        let prefix = LegacyAdvertisingPduPrefix::decode(&pdu).unwrap();
        assert_eq!(prefix.header.length().get(), length);
        assert_eq!(prefix.advertiser.kind(), LeDeviceAddressKind::Random);
        assert_eq!(prefix.advertiser.wire_bytes(), [1, 2, 3, 4, 5, 0xc6]);
    }
}
