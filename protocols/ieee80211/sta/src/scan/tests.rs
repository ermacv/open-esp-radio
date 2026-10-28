use super::{ScanObservation, ScanTable, best_matching_ssid};
use oer_ieee80211_wire::{
    ht::{HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN},
    scan::{
        EXTENDED_RATES_CAPACITY, HE_CAPABILITY_IE_CAPACITY, HE_OPERATION_IE_CAPACITY,
        HtSecondaryChannel, RSN_IE_CAPACITY, RSNXE_CAPACITY, ScanRecord, ScanRecordError,
        WMM_IE_CAPACITY, he_default_packet_extension_duration,
        he_extended_range_single_user_disabled, parse_management,
    },
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
fn get_returns_the_record_an_observation_names() {
    let mut table = ScanTable::<2>::new();
    let mut frame = [0_u8; 42];
    frame[0] = 0x80;
    frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
    assert!(table.get(0).is_none());
    let ScanObservation::Inserted { index } = table.observe_management(&frame, 6, -40, None) else {
        panic!("the first observation is inserted");
    };
    frame[21] = 2;
    let ScanObservation::Inserted { index: second } =
        table.observe_management(&frame, 6, -50, None)
    else {
        panic!("a second BSSID is inserted");
    };
    assert_eq!(table.get(index).unwrap().bssid.get(), [2, 0, 0, 0, 0, 1]);
    assert_eq!(table.get(second).unwrap().rssi, -50);
    assert!(table.get(2).is_none());
    assert!(table.get(usize::MAX).is_none());
    table.clear();
    assert!(table.get(index).is_none());
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
        table.observations().next().unwrap().received_at,
        Some(Instant::from_micros(1_000))
    );
    table.observe_management(&frame, 1, -60, Some(Instant::from_micros(2_500)));
    assert_eq!(
        table.observations().next().unwrap().received_at,
        Some(Instant::from_micros(2_500))
    );
    // The record's TSF comes from the latest frame, so does its reception
    // time, even when the receiver has none for it.
    table.observe_management(&frame, 1, -60, None);
    assert_eq!(table.observations().next().unwrap().received_at, None);
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
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new([2, 2, 3, 4, 5, 6]).unwrap()
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

/// `rich_frame(8)` with the Order bit set and an HT Control field after the
/// MAC header, as an HT or HE access point may send it.
fn ht_control_beacon() -> std::vec::Vec<u8> {
    let mut frame = rich_frame(8);
    frame[1] |= 0x80;
    frame.splice(24..24, [0x0d, 0xf0, 0xad, 0x0b]);
    frame
}

#[test]
fn a_beacon_with_ht_control_parses_its_fixed_fields_after_it() {
    let plain = parse_management(&rich_frame(8), 1, -40).unwrap();
    let record = parse_management(&ht_control_beacon(), 1, -40).unwrap();
    assert_eq!(record.ssid_bytes(), b"test");
    assert_eq!(record.beacon_interval_tu, 100);
    assert_eq!(record.timestamp.as_micros(), 0x0123_4567_89ab_cdef);
    assert_eq!(record.capability_info, 0x0011);
    assert!(!record.information_elements_truncated);
    assert_eq!(record, plain);
}

#[test]
fn a_beacon_whose_ht_control_leaves_no_fixed_fields_is_ignored() {
    let mut frame = ht_control_beacon();
    // The header with its HT Control field and eleven of the twelve fixed
    // bytes.
    frame.truncate(24 + 4 + 11);
    assert!(parse_management(&frame, 1, -40).is_none());
}

#[test]
fn a_management_frame_of_another_protocol_version_is_ignored() {
    let mut frame = rich_frame(8);
    frame[0] |= 0x01;
    assert!(parse_management(&frame, 1, -40).is_none());
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
    let mut record = ScanRecord::new(
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    );
    let mut capability = [0; HT_CAPABILITY_IE_LEN];
    capability[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    let mut operation = [0; HT_OPERATION_IE_LEN];
    operation[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    record.set_ht_capability_ie(Some(capability));
    record.set_ht_operation_ie(Some(operation));
    assert_eq!(
        record.ht40_secondary_channel(),
        Some(HtSecondaryChannel::Above)
    );

    operation[3] = 0x07;
    record.set_ht_operation_ie(Some(operation));
    assert_eq!(
        record.ht40_secondary_channel(),
        Some(HtSecondaryChannel::Below)
    );
    capability[2] = 0;
    record.set_ht_capability_ie(Some(capability));
    assert_eq!(record.ht40_secondary_channel(), None);
}

#[test]
fn ht_short_guard_intervals_are_read_from_capability_info() {
    let mut record = ScanRecord::new(
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    );
    let mut capability = [0; HT_CAPABILITY_IE_LEN];
    capability[0..4].copy_from_slice(&[45, 26, 1 << 5, 0]);
    record.set_ht_capability_ie(Some(capability));
    assert!(record.supports_ht_short_guard_interval_20mhz());
    assert!(!record.supports_ht_short_guard_interval_40mhz());

    capability[2] = 1 << 6;
    record.set_ht_capability_ie(Some(capability));
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
    assert_eq!(
        table
            .records()
            .next()
            .unwrap()
            .ht_operation_ie_bytes()
            .unwrap()[4]
            & 0x03,
        1
    );

    frame[49] = 0;
    assert_eq!(
        table.observe_management(&frame, 11, -60, None),
        ScanObservation::Updated { index: 0 }
    );
    assert_eq!(
        table
            .records()
            .next()
            .unwrap()
            .ht_operation_ie_bytes()
            .unwrap()[4]
            & 0x03,
        0
    );
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
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
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
        record.set_ssid(b"test").unwrap();
        record.rssi = rssi;
        record.channel = channel;
    }
    assert_eq!(best_matching_ssid(&records, b"test").unwrap().channel, 6);
}

/// A beacon (or, with `subtype` 5, a probe response) carrying every element
/// `parse_management` retains.
fn rich_frame(subtype: u8) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; 36];
    frame[0] = subtype << 4;
    frame[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    frame[24..32].copy_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34] = 0x11;
    frame.extend_from_slice(&[0, 4, b't', b'e', b's', b't']);
    frame.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    frame.extend_from_slice(&[3, 1, 6]);
    frame.extend_from_slice(&[42, 1, 0x02]);
    frame.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    let mut ht_capability = [0_u8; 28];
    ht_capability[..6].copy_from_slice(&[45, 26, 0x6e, 0x10, 0x03, 0xff]);
    ht_capability[9] = 1;
    frame.extend_from_slice(&ht_capability);
    let mut ht_operation = [0_u8; 24];
    ht_operation[..5].copy_from_slice(&[61, 22, 6, 0x05, 0x01]);
    frame.extend_from_slice(&ht_operation);
    frame.extend_from_slice(&[
        48, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2,
        0x80, 0,
    ]);
    frame.extend_from_slice(&[244, 1, 0x20]);
    frame.extend_from_slice(&[255, 7, 35, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
    frame.extend_from_slice(&[255, 7, 36, 0xf4, 0x3f, 0x01, 0x05, 0xfd, 0xff]);
    frame.extend_from_slice(&[
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x72, 0x32, 47, 0,
    ]);
    frame.extend_from_slice(&[221, 6, 0x00, 0x50, 0xf2, 0x01, 1, 0]);
    frame
}

fn scan_seeds() -> std::vec::Vec<std::vec::Vec<u8>> {
    let mut beacon = std::vec![0_u8; 47];
    beacon[0] = 0x80;
    beacon[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    beacon[34] = 0x10;
    beacon[36..47].copy_from_slice(&[0, 4, b't', b'e', b's', b't', 3, 1, 11, 48, 0]);
    let mut hidden = std::vec![0_u8; 38];
    hidden[0] = 0x80;
    hidden[16..22].copy_from_slice(&[2, 2, 3, 4, 5, 6]);
    std::vec![
        rich_frame(8),
        rich_frame(5),
        ht_control_beacon(),
        beacon,
        hidden
    ]
}

/// Read every accessor of a parsed record.
fn read_record(record: &ScanRecord) {
    let _ = record.ssid_bytes();
    let _ = crate::security::matches_security(
        record,
        oer_ieee80211_wire::security::LinkProtection::Open,
    );
    let _ = crate::security::matches_security(
        record,
        oer_ieee80211_wire::security::LinkProtection::Ccmp,
    );
    let _ = record.supported_rates_bytes();
    let _ = record.extended_supported_rates_bytes();
    let _ = record.erp_information();
    let _ = record.ht_capability_ie_bytes();
    let _ = record
        .ht_peer_capabilities()
        .map(|peer| peer.highest_rx_mcs());
    let _ = record.supports_ht_duplicate_mcs32();
    let _ = record.ht_operation_ie_bytes();
    let _ = record.supports_ht_short_guard_interval_20mhz();
    let _ = record.supports_ht_short_guard_interval_40mhz();
    let _ = record.ht40_secondary_channel();
    let _ = record.he_capability_ie_bytes();
    let _ = he_default_packet_extension_duration(record.he_operation_ie_bytes());
    let _ = he_extended_range_single_user_disabled(record.he_operation_ie_bytes());
    let _ = record.wmm_ie_bytes();
    let _ = record.wmm_parameters();
    let _ = oer_ieee80211_wire::security::rsn::RsnElement::parse(record.rsn_ie_bytes());
    let _ = record.rsnxe_bytes();
}

fn parse_management_never_panics(count: usize) {
    let seeds = scan_seeds();
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(std::vec::Vec::as_slice).collect();
    oer_ieee80211_wire::fuzz::inputs(&seeds, count, 320, |frame| {
        if let Some(record) = parse_management(frame, 6, -40) {
            read_record(&record);
            let _ = best_matching_ssid([&record], b"test");
        }
    });
}

#[test]
fn parse_management_never_panics_on_received_bytes() {
    parse_management_never_panics(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn parse_management_never_panics_on_received_bytes_full() {
    parse_management_never_panics(oer_ieee80211_wire::fuzz::FULL);
}

fn observe_management_never_panics(count: usize) {
    let seeds = scan_seeds();
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(std::vec::Vec::as_slice).collect();
    let mut table = ScanTable::<4>::new();
    let mut rssi = 0_i8;
    oer_ieee80211_wire::fuzz::inputs(&seeds, count, 320, |frame| {
        rssi = rssi.wrapping_sub(7);
        let _ = table.observe_management(frame, 1, rssi, Some(Instant::from_micros(1)));
        if table.summary().observed_frames.is_multiple_of(64) {
            table.clear();
        }
    });
    for record in table.records() {
        read_record(record);
    }
}

#[test]
fn observe_management_never_panics_on_received_bytes() {
    observe_management_never_panics(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn observe_management_never_panics_on_received_bytes_full() {
    observe_management_never_panics(oer_ieee80211_wire::fuzz::FULL);
}

fn he_operation_never_panics(count: usize) {
    let seeds: [&[u8]; 3] = [
        &[255, 4, 36, 0b1010_1100, 0, 0],
        &[255, 4, 36, 0, 0, 1],
        &[255, 7, 36, 0xf4, 0x3f, 0x01, 0x05, 0xfd, 0xff],
    ];
    oer_ieee80211_wire::fuzz::inputs(&seeds, count, 16, |element| {
        let _ = he_default_packet_extension_duration(element);
        let _ = he_extended_range_single_user_disabled(element);
    });
}

#[test]
fn he_operation_readers_never_panic_on_received_bytes() {
    he_operation_never_panics(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn he_operation_readers_never_panic_on_received_bytes_full() {
    he_operation_never_panics(oer_ieee80211_wire::fuzz::FULL);
}

#[test]
fn a_rich_frame_keeps_every_retained_element() {
    let record = parse_management(&rich_frame(8), 1, -40).unwrap();
    assert!(!record.information_elements_truncated);
    assert_eq!(record.channel, 6);
    assert_eq!(record.supported_rates_bytes().len(), 8);
    assert_eq!(
        record.extended_supported_rates_bytes(),
        &[0x30, 0x48, 0x60, 0x6c]
    );
    assert_eq!(record.erp_information(), Some(0x02));
    assert!(record.supports_ht_duplicate_mcs32());
    assert_eq!(
        record.ht40_secondary_channel(),
        Some(HtSecondaryChannel::Above)
    );
    assert_eq!(record.rsn_ie_bytes().len(), 22);
    // Mixed WPA/WPA2 advertisements match no exact mode.
    assert!(!crate::security::matches_security(
        &record,
        oer_ieee80211_wire::security::LinkProtection::Ccmp
    ));
    assert!(record.legacy_wpa);
    assert_eq!(record.rsnxe_bytes(), &[244, 1, 0x20]);
    assert_eq!(record.he_capability_ie_bytes().len(), 9);
    assert_eq!(
        he_default_packet_extension_duration(record.he_operation_ie_bytes()),
        Some(4)
    );
    assert_eq!(
        he_extended_range_single_user_disabled(record.he_operation_ie_bytes()),
        Some(true)
    );
    assert!(record.wmm_parameters().is_some());
}

#[test]
fn an_oversized_element_marks_the_record_truncated_and_keeps_the_stored_one() {
    let mut frame = rich_frame(8);
    // A 30-byte RSNXE exceeds its 16-byte capacity.
    frame.extend_from_slice(&[244, 28]);
    frame.extend_from_slice(&[0; 28]);
    let record = parse_management(&frame, 1, -40).unwrap();
    assert!(record.information_elements_truncated);
    assert_eq!(record.rsnxe_bytes(), &[244, 1, 0x20]);
}

#[test]
fn a_setter_refuses_a_value_longer_than_its_buffer_and_keeps_the_record() {
    type Setter = fn(&mut ScanRecord, &[u8]) -> Result<(), ScanRecordError>;
    type Getter = fn(&ScanRecord) -> &[u8];
    let setters: [(Setter, Getter, usize); 8] = [
        (ScanRecord::set_ssid, ScanRecord::ssid_bytes, 32),
        (
            ScanRecord::set_supported_rates,
            ScanRecord::supported_rates_bytes,
            8,
        ),
        (
            ScanRecord::set_extended_supported_rates,
            ScanRecord::extended_supported_rates_bytes,
            EXTENDED_RATES_CAPACITY,
        ),
        (
            ScanRecord::set_he_capability_ie,
            ScanRecord::he_capability_ie_bytes,
            HE_CAPABILITY_IE_CAPACITY,
        ),
        (
            ScanRecord::set_he_operation_ie,
            ScanRecord::he_operation_ie_bytes,
            HE_OPERATION_IE_CAPACITY,
        ),
        (
            ScanRecord::set_wmm_ie,
            ScanRecord::wmm_ie_bytes,
            WMM_IE_CAPACITY,
        ),
        (
            ScanRecord::set_rsn_ie,
            ScanRecord::rsn_ie_bytes,
            RSN_IE_CAPACITY,
        ),
        (
            ScanRecord::set_rsnxe,
            ScanRecord::rsnxe_bytes,
            RSNXE_CAPACITY,
        ),
    ];
    let value = [0xa5_u8; 300];
    let empty = ScanRecord::new(
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    );
    for (set, get, capacity) in setters {
        let mut record = empty;
        // A value that fills the buffer is held whole.
        assert_eq!(set(&mut record, &value[..capacity]), Ok(()));
        assert_eq!(get(&record), &value[..capacity]);
        let full = record;
        // One byte more, or more than a `u8` length, is refused, never
        // truncated, and the record keeps what it held.
        for length in [capacity + 1, 256, value.len()] {
            assert_eq!(
                set(&mut record, &value[..length]),
                Err(ScanRecordError::TooLong { length, capacity })
            );
            assert_eq!(record, full);
        }
        // A shorter value replaces the longer one whole.
        assert_eq!(set(&mut record, &value[..1]), Ok(()));
        assert_eq!(get(&record), &value[..1]);
        // An empty value leaves no byte of the earlier ones behind.
        assert_eq!(set(&mut record, &[]), Ok(()));
        assert_eq!(record, empty);
    }
}

#[test]
fn a_full_table_drops_a_new_bss_and_keeps_its_records() {
    let mut table = ScanTable::<1>::new();
    let mut frame = rich_frame(8);
    assert_eq!(
        table.observe_management(&frame, 1, -40, None),
        ScanObservation::Inserted { index: 0 }
    );
    frame[21] = 7;
    assert_eq!(
        table.observe_management(&frame, 1, -40, None),
        ScanObservation::Dropped
    );
    assert_eq!(table.summary().records, 1);
    assert_eq!(table.summary().dropped_unique_bss, 1);
    assert_eq!(table.records().count(), 1);
}
