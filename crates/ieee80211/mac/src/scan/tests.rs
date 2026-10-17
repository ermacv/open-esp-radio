use super::{
    HtSecondaryChannel, ScanObservation, ScanRecord, ScanTable, best_matching_ssid,
    he_default_packet_extension_duration, he_extended_range_single_user_disabled, parse_management,
};
use oer_time::Instant;

#[test]
fn a_zero_bssid_cannot_enter_the_scan_table() {
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert!(parse_management(&frame, 1, -40).is_none());
    let mut table = ScanTable::<1>::new();
    assert_eq!(
        table.observe_management(&frame, 1, -40, None),
        ScanObservation::Ignored
    );
    assert_eq!(table.summary().records, 0);
}

#[test]
fn vacant_scan_slots_never_appear_as_observations() {
    let mut table = ScanTable::<2>::new();
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert_eq!(table.records().count(), 0);
    for index in 0..2 {
        frame[21] = index as u8 + 1;
        assert_eq!(
            table.observe_management(&frame, 6, -40, None),
            ScanObservation::Inserted { index }
        );
    }
    assert_eq!(table.records().count(), 2);
    assert!(best_matching_ssid(table.records(), b"test").is_some());
    table.clear();
    assert_eq!(table.records().count(), 0);
    assert!(best_matching_ssid(table.records(), b"test").is_none());
    assert_eq!(
        table.observe_management(&frame, 6, -40, None),
        ScanObservation::Inserted { index: 0 }
    );
    assert_eq!(table.records().count(), 1);
    assert_eq!(
        table.records().next().unwrap().bssid.get(),
        [2, 0, 0, 0, 0, 2]
    );
}

#[test]
fn a_group_bssid_cannot_enter_the_scan_table() {
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[3, 2, 3, 4, 5, 6]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert!(parse_management(&frame, 1, -40).is_none());
    frame[16] = 2;
    assert_eq!(
        parse_management(&frame, 1, -40).unwrap().bssid.get(),
        [2, 2, 3, 4, 5, 6]
    );
}

#[test]
fn a_record_keeps_the_time_of_its_latest_observation() {
    let mut table = ScanTable::<1>::new();
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    table.observe_management(&frame, 1, -70, Some(Instant::from_micros(1_000)));
    assert_eq!(
        table.records().next().unwrap().received_at,
        Some(Instant::from_micros(1_000))
    );
    table.observe_management(&frame, 1, -60, Some(Instant::from_micros(2_500)));
    assert_eq!(
        table.records().next().unwrap().received_at,
        Some(Instant::from_micros(2_500))
    );
    // The record's TSF comes from the latest frame, so does its reception
    // time, even when the receiver has none for it.
    table.observe_management(&frame, 1, -60, None);
    assert_eq!(table.records().next().unwrap().received_at, None);
}

#[test]
fn extracts_default_pe_duration_from_complete_he_operation_ie() {
    assert_eq!(
        he_default_packet_extension_duration(&[255, 4, 36, 0b1010_1100, 0, 0]),
        Some(4)
    );
    assert_eq!(
        he_default_packet_extension_duration(&[255, 4, 35, 4, 0, 0]),
        None
    );
    assert_eq!(he_default_packet_extension_duration(&[255, 1, 36]), None);
}

#[test]
fn extracts_ersu_argument_from_complete_he_operation_ie() {
    assert_eq!(
        he_extended_range_single_user_disabled(&[255, 4, 36, 0, 0, 1]),
        Some(true)
    );
    assert_eq!(
        he_extended_range_single_user_disabled(&[255, 4, 36, 0, 0, 0]),
        Some(false)
    );
    assert_eq!(
        he_extended_range_single_user_disabled(&[255, 2, 36, 0]),
        None
    );
}

#[test]
fn parses_beacon_into_owned_bounded_record() {
    let mut frame = [0_u8; 64];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[24..32].copy_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34] = 0x10;
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    frame[42..45].copy_from_slice(&[3, 1, 11]);
    frame[45..47].copy_from_slice(&[48, 0]);
    let record = parse_management(&frame[..47], 3, -42).unwrap();
    assert_eq!(record.ssid_bytes(), b"test");
    assert_eq!(
        record.bssid,
        crate::address::Bssid::new(
            crate::address::IndividualAddress::new([2, 2, 3, 4, 5, 6]).unwrap()
        )
        .unwrap()
    );
    assert_eq!(record.channel, 11);
    assert_eq!(record.rssi, -42);
    assert_eq!(record.timestamp.as_micros(), 0x0123_4567_89ab_cdef);
    assert_eq!(record.beacon_interval_tu, 100);
    assert!(record.privacy);
    assert!(record.rsn);
}

#[test]
fn retains_erp_use_protection_without_confusing_the_element_header() {
    let mut frame = [0_u8; 43];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[36..40].copy_from_slice(&[0, 2, b'a', b'p']);
    frame[40..43].copy_from_slice(&[42, 1, 0x03]);
    let record = parse_management(&frame, 6, -42).unwrap();
    assert_eq!(record.erp_information(), Some(0x03));
}

#[test]
fn ht40_geometry_requires_matching_capability_and_operation() {
    let mut record = ScanRecord {
        channel: 6,
        ht_capability_ie_present: true,
        ht_operation_ie_present: true,
        ..ScanRecord::new(
            crate::address::Bssid::new(
                crate::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            )
            .unwrap(),
            6,
            -40,
        )
    };
    record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    assert_eq!(
        record.ht40_secondary_channel(),
        Some(HtSecondaryChannel::Above)
    );

    record.ht_operation_ie[3] = 0x07;
    assert_eq!(
        record.ht40_secondary_channel(),
        Some(HtSecondaryChannel::Below)
    );
    record.ht_capability_ie[2] = 0;
    assert_eq!(record.ht40_secondary_channel(), None);
}

#[test]
fn ht_short_guard_intervals_are_read_from_capability_info() {
    let mut record = ScanRecord {
        ht_capability_ie_present: true,
        ..ScanRecord::new(
            crate::address::Bssid::new(
                crate::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            )
            .unwrap(),
            6,
            -40,
        )
    };
    record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 1 << 5, 0]);
    assert!(record.supports_ht_short_guard_interval_20mhz());
    assert!(!record.supports_ht_short_guard_interval_40mhz());

    record.ht_capability_ie[2] = 1 << 6;
    assert!(!record.supports_ht_short_guard_interval_20mhz());
    assert!(record.supports_ht_short_guard_interval_40mhz());
}

#[test]
fn table_deduplicates_by_bssid_and_retains_latest_record() {
    let mut table = ScanTable::<1>::new();
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert_eq!(
        table.observe_management(&frame, 1, -70, None),
        ScanObservation::Inserted { index: 0 }
    );
    assert_eq!(
        table.observe_management(&frame, 1, -30, None),
        ScanObservation::Updated { index: 0 }
    );
    assert_eq!(table.records().next().unwrap().rssi, -30);
    assert_eq!(
        table.observe_management(&frame, 1, -60, None),
        ScanObservation::Updated { index: 0 }
    );
    assert_eq!(table.records().next().unwrap().rssi, -60);
    assert_eq!(table.summary().observed_frames, 3);
}

#[test]
fn weaker_latest_beacon_replaces_stale_ht_protection() {
    let mut table = ScanTable::<1>::new();
    let mut frame = [0_u8; 69];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    frame[42..45].copy_from_slice(&[3, 1, 11]);
    frame[45..69].copy_from_slice(&[
        61, 22, 11, 0x07, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);

    assert_eq!(
        table.observe_management(&frame, 11, -30, None),
        ScanObservation::Inserted { index: 0 }
    );
    assert_eq!(table.records().next().unwrap().ht_operation_ie[4] & 0x03, 1);

    frame[49] = 0;
    assert_eq!(
        table.observe_management(&frame, 11, -60, None),
        ScanObservation::Updated { index: 0 }
    );
    assert_eq!(table.records().next().unwrap().ht_operation_ie[4] & 0x03, 0);
    assert_eq!(table.records().next().unwrap().rssi, -60);
}

#[test]
fn latest_hidden_beacon_preserves_probe_response_ssid() {
    let mut table = ScanTable::<1>::new();
    let mut probe = [0_u8; 42];
    probe[0] = 0x50;
    probe[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    probe[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert_eq!(
        table.observe_management(&probe, 1, -40, None),
        ScanObservation::Inserted { index: 0 }
    );

    let mut beacon = [0_u8; 38];
    beacon[0] = 0x80;
    beacon[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    beacon[36..38].copy_from_slice(&[0, 0]);
    assert_eq!(
        table.observe_management(&beacon, 1, -55, None),
        ScanObservation::Updated { index: 0 }
    );
    assert_eq!(table.records().next().unwrap().ssid_bytes(), b"test");
    assert_eq!(table.records().next().unwrap().rssi, -55);
}

#[test]
fn strongest_exact_ssid_ignores_invalid_channel() {
    let mut records = [ScanRecord::new(
        crate::address::Bssid::new(
            crate::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    ); 3];
    for (record, rssi, channel) in [(-70, 1), (-25, 6), (-10, 0)]
        .into_iter()
        .zip(&mut records)
        .map(|((rssi, channel), record)| (record, rssi, channel))
    {
        record.ssid[..4].copy_from_slice(b"test");
        record.ssid_len = 4;
        record.rssi = rssi;
        record.channel = channel;
    }
    assert_eq!(best_matching_ssid(&records, b"test").unwrap().channel, 6);
}
