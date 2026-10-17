use super::*;

const DESTINATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
const SOURCE: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const BSSID: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
const AP_MAC: [u8; 6] = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45];

const fn ethernet(destination: [u8; 6]) -> [u8; ETHERNET_HEADER_LEN] {
    [
        destination[0],
        destination[1],
        destination[2],
        destination[3],
        destination[4],
        destination[5],
        SOURCE[0],
        SOURCE[1],
        SOURCE[2],
        SOURCE[3],
        SOURCE[4],
        SOURCE[5],
        0x08,
        0x00,
    ]
}

#[test]
fn duplicate_history_query_never_accepts_fragment_state() {
    let mut filter = RxDuplicateFilter::new();
    assert!(!filter.is_duplicate(false, 0x1230, None));
    let ordinary_history = filter;

    assert!(filter.is_known_duplicate(true, 0x1230, None));
    assert!(!filter.is_known_duplicate(true, 0x1240, None));
    assert!(!filter.is_known_duplicate(false, 0x1230, None));
    assert_eq!(filter, ordinary_history);
}

#[test]
fn station_qos_header_matches_the_recovered_plan() {
    let plan = plan_data_encapsulation(
        DataInterfaceRole::Station,
        BSSID,
        AP_MAC,
        ethernet(DESTINATION),
        WmmUserPriority::UP7,
        true,
        false,
    );
    assert_eq!(plan.header_len, 26);
    assert_eq!(&plan.header[..2], &[0x88, 0x01]);
    assert_eq!(&plan.header[4..10], &BSSID);
    assert_eq!(&plan.header[10..16], &SOURCE);
    assert_eq!(&plan.header[16..22], &DESTINATION);
    assert_eq!(&plan.header[24..26], &[7, 0]);
    assert_eq!(plan.access_category, WmmAccessCategory::Voice);
    assert_eq!(plan.he_control, DataHeControl::Disabled);
}

#[test]
fn hardware_bsr_sets_order_without_moving_dma_payload() {
    let plan = plan_data_encapsulation_with_he_control(
        DataInterfaceRole::Station,
        BSSID,
        AP_MAC,
        ethernet(DESTINATION),
        WmmUserPriority::UP0,
        true,
        false,
        DataHeControl::HardwareGeneratedBufferStatusReport,
    )
    .unwrap();
    assert_eq!(&plan.header[..2], &[0x88, 0x81]);
    assert_eq!(plan.header_len, IEEE80211_QOS_DATA_HEADER_LEN as u8);
    assert_eq!(plan.dma_header_len(), IEEE80211_QOS_DATA_HEADER_LEN);
    assert_eq!(plan.he_control.inserted_air_len(), IEEE80211_HE_CONTROL_LEN);
    assert_eq!(
        plan.he_control,
        DataHeControl::HardwareGeneratedBufferStatusReport
    );
    assert!(
        plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            BSSID,
            AP_MAC,
            ethernet(DESTINATION),
            WmmUserPriority::UP0,
            false,
            false,
            DataHeControl::HardwareGeneratedBufferStatusReport,
        )
        .is_err()
    );
}

#[test]
fn station_decapsulation_reverses_from_ds_rfc1042_data() {
    let ethernet = ethernet(DESTINATION);
    let plan = plan_data_encapsulation(
        DataInterfaceRole::AccessPoint,
        BSSID,
        BSSID,
        ethernet,
        WmmUserPriority::UP0,
        false,
        false,
    );
    let header_length = usize::from(plan.header_len);
    let payload = [1, 2, 3, 4];
    let mut mpdu = [0_u8; 64];
    mpdu[..header_length].copy_from_slice(&plan.header[..header_length]);
    mpdu[header_length..header_length + LLC_SNAP_HEADER_LEN].copy_from_slice(&plan.llc_snap);
    mpdu[header_length + LLC_SNAP_HEADER_LEN..header_length + LLC_SNAP_HEADER_LEN + payload.len()]
        .copy_from_slice(&payload);
    let mpdu_length = header_length + LLC_SNAP_HEADER_LEN + payload.len();
    let mut output = [0_u8; 64];
    let decoded = decapsulate_data(
        DataInterfaceRole::Station,
        &mpdu[..mpdu_length],
        header_length,
        LLC_SNAP_HEADER_LEN + payload.len(),
        &mut output,
    )
    .unwrap();

    assert_eq!(decoded.destination, DESTINATION);
    assert_eq!(decoded.source, SOURCE);
    assert_eq!(decoded.ether_type, 0x0800);
    assert_eq!(
        &output[..decoded.ethernet_length],
        &[&ethernet[..], &payload].concat()
    );
}

#[test]
fn protected_station_decapsulation_accepts_a_separate_ccmp_header() {
    let payload = [1, 2, 3, 4];
    let mut mpdu = [0_u8; 64];
    mpdu[0] = IEEE80211_DATA;
    mpdu[1] = IEEE80211_FROM_DS | 0x40;
    mpdu[4..10].copy_from_slice(&DESTINATION);
    mpdu[10..16].copy_from_slice(&BSSID);
    mpdu[16..22].copy_from_slice(&SOURCE);
    let ccmp_offset = IEEE80211_LEGACY_DATA_HEADER_LEN;
    let llc_offset = ccmp_offset + 8;
    mpdu[ccmp_offset + 3] = 0x20;
    mpdu[llc_offset..llc_offset + LLC_SNAP_HEADER_LEN]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    mpdu[llc_offset + LLC_SNAP_HEADER_LEN..llc_offset + LLC_SNAP_HEADER_LEN + payload.len()]
        .copy_from_slice(&payload);
    let mpdu_length = llc_offset + LLC_SNAP_HEADER_LEN + payload.len();
    let mut output = [0_u8; 64];
    let decoded = decapsulate_data(
        DataInterfaceRole::Station,
        &mpdu[..mpdu_length],
        llc_offset,
        LLC_SNAP_HEADER_LEN + payload.len(),
        &mut output,
    )
    .unwrap();

    assert_eq!(decoded.ether_type, 0x0806);
    assert_eq!(&output[..6], &DESTINATION);
    assert_eq!(&output[6..12], &SOURCE);
    assert_eq!(&output[14..decoded.ethernet_length], &payload);

    let mut frames = decapsulate_data_frames(
        DataInterfaceRole::Station,
        &mpdu[..mpdu_length],
        llc_offset,
        LLC_SNAP_HEADER_LEN + payload.len(),
    )
    .unwrap();
    assert!(!frames.is_amsdu());
    let frame = frames.next().unwrap().unwrap();
    assert_eq!(frame.destination, DESTINATION);
    assert_eq!(frame.source, SOURCE);
    assert_eq!(frame.ether_type, 0x0806);
    assert_eq!(frame.payload, payload);
    assert!(frames.next().is_none());
}

#[test]
fn station_amsdu_iterator_removes_subframe_length_llc_and_padding() {
    let mut mpdu = [0_u8; 96];
    mpdu[0] = IEEE80211_QOS_DATA;
    mpdu[1] = IEEE80211_FROM_DS;
    mpdu[24] = IEEE80211_QOS_AMSDU_PRESENT;
    let mut offset = IEEE80211_QOS_DATA_HEADER_LEN;

    mpdu[offset..offset + 6].copy_from_slice(&DESTINATION);
    mpdu[offset + 6..offset + 12].copy_from_slice(&SOURCE);
    mpdu[offset + 12..offset + 14].copy_from_slice(&10_u16.to_be_bytes());
    mpdu[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    mpdu[offset + 22..offset + 24].copy_from_slice(&[1, 2]);
    offset += 24;

    mpdu[offset..offset + 6].copy_from_slice(&[0xff; 6]);
    mpdu[offset + 6..offset + 12].copy_from_slice(&SOURCE);
    mpdu[offset + 12..offset + 14].copy_from_slice(&11_u16.to_be_bytes());
    mpdu[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    mpdu[offset + 22..offset + 25].copy_from_slice(&[3, 4, 5]);
    offset += 25;

    let mut subframes = amsdu_subframes(
        DataInterfaceRole::Station,
        &mpdu[..offset],
        IEEE80211_QOS_DATA_HEADER_LEN,
        offset - IEEE80211_QOS_DATA_HEADER_LEN,
    )
    .unwrap();
    let first = subframes.next().unwrap().unwrap();
    assert_eq!(first.destination, DESTINATION);
    assert_eq!(first.source, SOURCE);
    assert_eq!(first.ether_type, 0x0800);
    assert_eq!(first.payload, &[1, 2]);
    let second = subframes.next().unwrap().unwrap();
    assert_eq!(second.destination, [0xff; 6]);
    assert_eq!(second.ether_type, 0x0806);
    assert_eq!(second.payload, &[3, 4, 5]);
    assert!(subframes.next().is_none());

    let mut frames = decapsulate_data_frames(
        DataInterfaceRole::Station,
        &mpdu[..offset],
        IEEE80211_QOS_DATA_HEADER_LEN,
        offset - IEEE80211_QOS_DATA_HEADER_LEN,
    )
    .unwrap();
    assert!(frames.is_amsdu());
    let unified_first = frames.next().unwrap().unwrap();
    assert_eq!(unified_first.destination, DESTINATION);
    assert_eq!(unified_first.payload, &[1, 2]);
    let unified_second = frames.next().unwrap().unwrap();
    assert_eq!(unified_second.destination, [0xff; 6]);
    assert_eq!(unified_second.ether_type, 0x0806);
    assert_eq!(unified_second.payload, &[3, 4, 5]);
    assert!(frames.next().is_none());

    // Payload iteration is shared; only the DS direction is role policy.
    let mut ap_mpdu = mpdu;
    ap_mpdu[1] = IEEE80211_TO_DS;
    let mut ap_frames = decapsulate_data_frames(
        DataInterfaceRole::AccessPoint,
        &ap_mpdu[..offset],
        IEEE80211_QOS_DATA_HEADER_LEN,
        offset - IEEE80211_QOS_DATA_HEADER_LEN,
    )
    .unwrap();
    assert!(ap_frames.is_amsdu());
    assert_eq!(ap_frames.next().unwrap().unwrap().payload, &[1, 2]);
    assert_eq!(ap_frames.next().unwrap().unwrap().payload, &[3, 4, 5]);
    assert!(ap_frames.next().is_none());

    let mut output = [0; 32];
    let length = decapsulate_amsdu_subframe(second, &mut output).unwrap();
    assert_eq!(length, 17);
    assert_eq!(&output[..6], &[0xff; 6]);
    assert_eq!(&output[12..17], &[0x08, 0x06, 3, 4, 5]);
}

#[test]
fn amsdu_bit_flip_cannot_inject_a_second_msdu() {
    for (role, direction) in [
        (DataInterfaceRole::Station, IEEE80211_FROM_DS),
        (DataInterfaceRole::AccessPoint, IEEE80211_TO_DS),
    ] {
        for protected in [false, true] {
            let mut mpdu = [0_u8; 96];
            mpdu[0] = IEEE80211_QOS_DATA;
            mpdu[1] = direction | if protected { 0x40 } else { 0 };
            mpdu[4..10].copy_from_slice(&DESTINATION);
            mpdu[10..16].copy_from_slice(&BSSID);
            mpdu[16..22].copy_from_slice(&SOURCE);
            let offset = IEEE80211_QOS_DATA_HEADER_LEN + if protected { 8 } else { 0 };

            // An ordinary LLC/SNAP MSDU whose attacker-controlled data also
            // forms two valid A-MSDU subframes. Flipping only the outer QoS
            // bit must not expose the injected second Ethernet packet.
            let payload = &mut mpdu[offset..offset + 48];
            payload[..8].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
            payload[12..14].copy_from_slice(&10_u16.to_be_bytes());
            payload[14..22].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
            payload[22..24].copy_from_slice(&[1, 2]);
            payload[24..30].copy_from_slice(&DESTINATION);
            payload[30..36].copy_from_slice(&SOURCE);
            payload[36..38].copy_from_slice(&10_u16.to_be_bytes());
            payload[38..46].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x06]);
            payload[46..48].copy_from_slice(&[0xde, 0xad]);

            let mut ordinary = decapsulate_data_frames(role, &mpdu, offset, 48).unwrap();
            assert!(!ordinary.is_amsdu());
            assert_eq!(
                ordinary.next().unwrap().unwrap().payload,
                &mpdu[offset + 8..offset + 48]
            );
            assert!(ordinary.next().is_none());

            mpdu[24] = IEEE80211_QOS_AMSDU_PRESENT;
            assert!(matches!(
                decapsulate_data_frames(role, &mpdu, offset, 48),
                Err(DataDecapError::InvalidAmsduDestination)
            ));
            assert!(matches!(
                amsdu_subframes(role, &mpdu, offset, 48),
                Err(DataDecapError::InvalidAmsduDestination)
            ));

            // A real aggregate using the same body remains decodable. The
            // refusal is specific to the LLC-aliasing destination.
            mpdu[offset..offset + 6].copy_from_slice(&DESTINATION);
            let mut genuine = decapsulate_data_frames(role, &mpdu, offset, 48).unwrap();
            assert!(genuine.is_amsdu());
            assert_eq!(genuine.next().unwrap().unwrap().payload, [1, 2]);
            assert_eq!(genuine.next().unwrap().unwrap().payload, [0xde, 0xad]);
            assert!(genuine.next().is_none());
        }
    }
}

#[test]
fn decapsulation_rejects_role_mismatch_amsdu_and_non_snap_payload() {
    let mut mpdu = [0_u8; 40];
    mpdu[0] = IEEE80211_QOS_DATA;
    mpdu[1] = IEEE80211_FROM_DS;
    mpdu[24] = IEEE80211_QOS_AMSDU_PRESENT;
    mpdu[26..34].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);

    assert_eq!(
        plan_data_decapsulation(DataInterfaceRole::AccessPoint, &mpdu, 26, 8),
        Err(DataDecapError::RoleMismatch)
    );
    assert_eq!(
        plan_data_decapsulation(DataInterfaceRole::Station, &mpdu, 26, 8),
        Err(DataDecapError::AmsduUnsupported)
    );
    mpdu[24] = 0;
    mpdu[26] = 0;
    assert_eq!(
        plan_data_decapsulation(DataInterfaceRole::Station, &mpdu, 26, 8),
        Err(DataDecapError::InvalidLlcSnap)
    );
}
