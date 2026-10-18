use super::*;
use crate::air::test_support::mac;

#[test]
fn decodes_every_field_of_a_record() {
    let text = "100.0000125\t0x0028\t02:00:00:00:00:01\t02:00:00:00:00:02\t02:00:00:00:00:03\t44\t12\t0\t5\tTrue\t2\t7\t135\t\t\t77\tFalse\t9\t\t\t0a0b\n";
    let frames = parse(text, Payload::Include).unwrap();
    assert_eq!(
        frames,
        [AirFrame {
            time_micros: 100_000_012,
            kind: FrameKind(0x28),
            transmitter: Some(mac("02:00:00:00:00:01")),
            receiver: Some(mac("02:00:00:00:00:02")),
            destination: Some(mac("02:00:00:00:00:03")),
            duration_micros: Some(44),
            sequence: Some(12),
            fragment: Some(0),
            tid: Some(5),
            retry: Some(true),
            transmit_retries: Some(2),
            phy: Some(AirPhy::Ht),
            rate_kbps: Some(135_000),
            block_ack: None,
            mac_time_micros: Some(77),
            short_preamble: Some(false),
            ampdu_reference: Some(9),
            erp_information: None,
            ht_protection: None,
            payload: Some(vec![0x0a, 0x0b]),
        }]
    );
    assert!(frames[0].kind.is_data());
}

#[test]
fn control_frames_keep_absent_fields_absent() {
    let text = "5.5\t0x001c\t\t02:00:00:00:00:01\t\t60\t\t\t\t0\t\t4\t5.5\t\t\t\t\t\t\t\n\
                6\t0x0019\t02:00:00:00:00:02\t02:00:00:00:00:01\t\t0\t\t\t\t0\t\t7\t24\t4090\t7f00000000000000\t\t\t\t\t\n\
                7\t0x0008\t02:00:00:00:00:02\tff:ff:ff:ff:ff:ff\t\t0\t9\t0\t\t0\t\t4\t1\t\t\t\t\t\t0x02\t0x0003\n";
    let frames = parse(text, Payload::Omit).unwrap();
    assert_eq!(frames[0].kind, FrameKind::CTS);
    assert_eq!(frames[0].transmitter, None);
    assert_eq!(frames[0].duration_micros, Some(60));
    assert_eq!(frames[0].phy, Some(AirPhy::HrDsss));
    assert!(frames[0].phy.unwrap().is_dsss());
    assert_eq!(frames[0].rate_kbps, Some(5_500));
    assert_eq!(frames[2].erp_information, Some(0x02));
    assert_eq!(frames[2].ht_protection, Some(3));
    assert_eq!(
        frames[1].block_ack,
        Some(BlockAckBitmap {
            start_sequence: 4090,
            bitmap: [0x7f, 0, 0, 0, 0, 0, 0, 0],
        })
    );
}

#[test]
fn malformed_records_fail_the_capture() {
    let valid =
        "1\t0x0004\t02:00:00:00:00:01\tff:ff:ff:ff:ff:ff\t\t0\t3\t0\t\tFalse\t\t6\t6\t\t\t\t\t\t\t";
    parse(valid, Payload::Omit).unwrap();
    for broken in [
        valid.replacen("1\t", "NaN\t", 1),
        valid.replace("\tFalse", "\tmaybe"),
        valid.replace("02:00:00:00:00:01", "02:00:00:00:01"),
        valid.replace("\t3\t", "\tthree\t"),
        valid.replace("\t6\t6", "\t6"),
        valid.replace("\t6\t6", "\t6\t6.x"),
    ] {
        assert!(parse(&broken, Payload::Omit).is_err(), "{broken}");
    }
}

#[test]
fn epoch_parser_is_integer_and_microsecond_bounded() {
    assert_eq!(epoch_micros("12"), Some(12_000_000));
    assert_eq!(epoch_micros("12.3"), Some(12_300_000));
    assert_eq!(epoch_micros("12.345678999"), Some(12_345_678));
    assert_eq!(epoch_micros("broken"), None);
    assert_eq!(MacAddress::BROADCAST.to_string(), "ff:ff:ff:ff:ff:ff");
}

#[test]
fn beacon_rates_collect_the_basic_set_and_erp_protection() {
    let rates = parse_beacon_rates(
        "0x8b,0x96,0x82,0x84,0x0c,0x18,0x30,0x60\t0x6c,0x12,0x24,0x48\t0x00\n\
         0x8b,0x96,0x82,0x84,0x0c,0x18,0x30,0x60\t0x6c,0x12,0x24,0x48\t0x02\n",
    )
    .unwrap();
    assert_eq!(
        rates.basic_kbps.into_iter().collect::<Vec<_>>(),
        [1_000, 2_000, 5_500, 11_000]
    );
    assert!(rates.erp_use_protection);
    let ofdm = parse_beacon_rates("0x8c,0x12,0x98,0x24,0xb0,0x48,0x60,0x6c\t\t\n").unwrap();
    assert_eq!(
        ofdm.basic_kbps.into_iter().collect::<Vec<_>>(),
        [6_000, 12_000, 24_000]
    );
    assert!(!ofdm.erp_use_protection);
    assert!(parse_beacon_rates("").is_err());
    assert!(parse_beacon_rates("0x8b\t\n").is_err());
}
