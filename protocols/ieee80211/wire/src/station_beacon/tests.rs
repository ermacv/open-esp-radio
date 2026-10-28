extern crate std;

use super::*;

const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

fn beacon(tim: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; FIXED_BEACON_LENGTH];
    frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..32].copy_from_slice(&0x0102_0304_0506_0708_u64.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(tim);
    frame
}

#[test]
fn decodes_dtim_group_and_partial_virtual_bitmap_for_local_aid() {
    // AID 17 lives in byte two, bit one. Bitmap offset one is encoded as
    // two in bits 7:1 and therefore body bitmap byte zero names AID16..23.
    let frame = beacon(&[TIM_ELEMENT_ID, 4, 0, 3, 0x03, 0x02]);
    assert_eq!(
        parse_sta_beacon(&frame, BSSID, 17),
        Ok(StaBeaconObservation {
            timestamp_tsf: crate::tsf::TsfTimestamp::from_micros(0x0102_0304_0506_0708),
            interval_tu: 100,
            capability_information: 0x0431,
            tim: Some(StaTimObservation {
                dtim_count: 0,
                dtim_period: 3,
                unicast_buffered: true,
                group_buffered: true,
            }),
            protection: StaBeaconProtection::UNPROTECTED,
            edca: None,
            channel_switch: None,
        })
    );
}

#[test]
fn carries_the_wmm_parameter_element_of_the_beacon() {
    let mut elements = std::vec![TIM_ELEMENT_ID, 4, 0, 3, 0, 0];
    let wmm = [
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x07, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x62, 0x32, 47, 0,
    ];
    elements.extend_from_slice(&wmm);
    let observation = parse_sta_beacon(&beacon(&elements), BSSID, 1).unwrap();
    assert_eq!(
        observation.edca,
        crate::extensions::wmm::parse_wmm_parameter_element(&wmm)
    );
    assert_eq!(observation.edca.unwrap().parameter_set_count, 7);

    // A WMM Information element is not a parameter set.
    let information = beacon(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0, 1, 0x07]);
    assert_eq!(parse_sta_beacon(&information, BSSID, 1).unwrap().edca, None);
}

#[test]
fn rejects_foreign_or_structurally_incomplete_beacons() {
    let mut foreign = beacon(&[TIM_ELEMENT_ID, 4, 1, 3, 0, 0]);
    foreign[10] ^= 1;
    assert_eq!(
        parse_sta_beacon(&foreign, BSSID, 1),
        Err(StaBeaconError::ForeignBssid)
    );

    let malformed = beacon(&[TIM_ELEMENT_ID, 5, 1, 3, 0, 0]);
    assert_eq!(
        parse_sta_beacon(&malformed, BSSID, 1),
        Err(StaBeaconError::MalformedInformationElement)
    );
}

#[test]
fn decodes_erp_ht_and_he_protection_fields() {
    let mut elements = std::vec![ERP_ELEMENT_ID, 1, 0x07];
    let mut ht_operation = [0_u8; 24];
    ht_operation[..3].copy_from_slice(&[HT_OPERATION_ELEMENT_ID, 22, 6]);
    ht_operation[4] = 0x05;
    elements.extend_from_slice(&ht_operation);
    // HE Operation with TXOP Duration RTS Threshold 64 (bits 13:4).
    elements.extend_from_slice(&[
        EXTENSION_ELEMENT_ID,
        7,
        HE_OPERATION_EXTENSION_ID,
        0x00,
        0x04,
        0,
        5,
        0xfd,
        0xff,
    ]);
    let observation = parse_sta_beacon(&beacon(&elements), BSSID, 1).unwrap();
    assert_eq!(
        observation.protection,
        StaBeaconProtection {
            erp: ErpProtection::new(true, true),
            ht: HtProtectionMode::Nonmember,
            he_txop_rts_threshold: Some(64),
        }
    );

    let disabled = [
        EXTENSION_ELEMENT_ID,
        7,
        HE_OPERATION_EXTENSION_ID,
        0xf0,
        0x3f,
        0,
        5,
        0xfd,
        0xff,
    ];
    let observation = parse_sta_beacon(&beacon(&disabled), BSSID, 1).unwrap();
    assert_eq!(observation.protection, StaBeaconProtection::UNPROTECTED);
}

#[test]
fn a_beacon_carries_its_bss_s_channel_switch_announcement() {
    use crate::channel_switch::{ChannelSwitch, ChannelSwitchMode, SecondaryChannelOffset};

    let frame = beacon(&[37, 3, 1, 11, 4, 62, 1, 3]);
    assert_eq!(
        parse_sta_beacon(&frame, BSSID, 17).map(|observation| observation.channel_switch),
        Ok(Some(Ok(ChannelSwitch {
            mode: ChannelSwitchMode::StopTransmitting,
            channel_number: 11,
            count: 4,
            operating_class: None,
            secondary: SecondaryChannelOffset::Below,
        })))
    );
    // A reserved switch mode spoils the announcement, not the beacon: its
    // traffic indication still counts.
    let observation = parse_sta_beacon(
        &beacon(&[TIM_ELEMENT_ID, 4, 0, 3, 0, 0, 37, 3, 7, 11, 4]),
        BSSID,
        17,
    )
    .unwrap();
    assert_eq!(
        observation.channel_switch,
        Some(Err(crate::channel_switch::ChannelSwitchError::Malformed))
    );
    assert!(observation.tim.is_some());
}

fn parse_sta_beacon_never_panics(count: usize) {
    let mut protection = std::vec![ERP_ELEMENT_ID, 1, 0x07];
    let mut ht_operation = [0_u8; 24];
    ht_operation[..3].copy_from_slice(&[HT_OPERATION_ELEMENT_ID, 22, 6]);
    ht_operation[4] = 0x05;
    protection.extend_from_slice(&ht_operation);
    protection.extend_from_slice(&[
        EXTENSION_ELEMENT_ID,
        7,
        HE_OPERATION_EXTENSION_ID,
        0x00,
        0x04,
        0,
        5,
        0xfd,
        0xff,
    ]);
    let seeds = [
        beacon(&[TIM_ELEMENT_ID, 4, 0, 3, 0x03, 0x02]),
        beacon(&[TIM_ELEMENT_ID, 6, 1, 3, 0xfe, 0, 0, 0xff]),
        beacon(&protection),
        beacon(&[37, 3, 1, 11, 4, 62, 1, 3]),
        beacon(&[TIM_ELEMENT_ID, 4, 0, 3, 0, 0, 37, 3, 7, 11, 4]),
        beacon(&[60, 4, 0, 117, 40, 2]),
    ];
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(std::vec::Vec::as_slice).collect();
    let mut round = 0_usize;
    crate::fuzz::inputs(&seeds, count, 160, |mpdu| {
        round = round.wrapping_add(1);
        let association_id = [0, 1, 17, 2007, 0x3fff, 0xffff][round % 6];
        if let Ok(observation) = parse_sta_beacon(mpdu, BSSID, association_id)
            && let Some(Ok(announcement)) = observation.channel_switch
        {
            let _ = announcement.target(crate::channel::Band::Ghz2_4);
            let _ = announcement.target(crate::channel::Band::Ghz5);
        }
    });
}

#[test]
fn parse_sta_beacon_never_panics_on_received_bytes() {
    parse_sta_beacon_never_panics(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn parse_sta_beacon_never_panics_on_received_bytes_full() {
    parse_sta_beacon_never_panics(crate::fuzz::FULL);
}

#[test]
fn the_highest_association_id_reads_the_last_bit_of_its_octet() {
    // AID 2007 is bit seven of octet 250; a bitmap offset of 250 (encoded
    // 250 in bits 7:1) makes it the first bitmap octet.
    let frame = beacon(&[TIM_ELEMENT_ID, 4, 1, 3, 250, 0x80]);
    let tim = parse_sta_beacon(&frame, BSSID, 2007).unwrap().tim.unwrap();
    assert!(tim.unicast_buffered);
    assert!(!tim.group_buffered);
    // A TIM without a bitmap octet, or with DTIM period zero, is malformed.
    for tim in [
        &[TIM_ELEMENT_ID, 3, 0, 3, 0][..],
        &[TIM_ELEMENT_ID, 4, 0, 0, 0, 0],
    ] {
        assert_eq!(
            parse_sta_beacon(&beacon(tim), BSSID, 1),
            Err(StaBeaconError::MalformedTim)
        );
    }
    // A lone trailing octet cannot be an element header.
    let mut frame = beacon(&[]);
    frame.push(TIM_ELEMENT_ID);
    assert_eq!(
        parse_sta_beacon(&frame, BSSID, 1),
        Err(StaBeaconError::MalformedInformationElement)
    );
    assert_eq!(
        parse_sta_beacon(&frame[..FIXED_BEACON_LENGTH - 1], BSSID, 1),
        Err(StaBeaconError::Truncated)
    );
}
