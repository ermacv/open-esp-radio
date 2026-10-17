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
            timestamp_tsf: crate::tsf::TsfInstant::from_micros(0x0102_0304_0506_0708),
            interval_tu: 100,
            capability_information: 0x0431,
            tim: Some(StaTimObservation {
                dtim_count: 0,
                dtim_period: 3,
                unicast_buffered: true,
                group_buffered: true,
            }),
            protection: StaBeaconProtection::UNPROTECTED,
            channel_switch: None,
        })
    );
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
