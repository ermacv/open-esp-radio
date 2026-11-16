//! The HE20 advertisement claims nothing optional.

use super::*;
use oer_protocols_ieee80211_wire::he::{
    HeDcmConstellation, parse_he_capabilities, parse_he_peer_state,
};

#[test]
fn the_he20_element_is_one_stream_mcs9_both_ways_with_no_optional_capability() {
    assert_eq!(
        HE20,
        [
            0xff, 0x16, 0x23, // HE Capabilities
            0x00, 0x08, 0x00, 0x00, 0x00, 0x00, // MAC: TF MAC padding 16 us only
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
            0x00, // PHY: nominal packet padding 16 us only
            0xfd, 0xff, 0xfd, 0xff, // HE-MCS 0..9, one stream, both ways
        ]
    );
    let capability = parse_he_capabilities(&HE20).unwrap();
    assert!(capability.supports_bidirectional_mcs9());
    assert!(!capability.supports_ldpc_coding_in_payload());
    assert!(!capability.supports_one_ltf_800ns_gi());
    assert!(
        !capability.phy.stbc_receive_under_80_mhz() && !capability.phy.stbc_transmit_under_80_mhz()
    );
    assert_eq!(capability.dcm_receive(), HeDcmConstellation::NotSupported);
    assert_eq!(capability.dcm_transmit(), HeDcmConstellation::NotSupported);
    assert!(!capability.phy.triggered_su_beamforming_feedback());
    assert!(
        !capability.phy.triggered_cqi_feedback() && !capability.phy.non_triggered_cqi_feedback()
    );
    let mac = HeMacCapabilities::from_bytes([HE20[3], HE20[4], HE20[5], HE20[6], HE20[7], HE20[8]]);
    assert_eq!(
        mac.trigger_frame_mac_padding(),
        HeMacCapabilities::TRIGGER_FRAME_MAC_PADDING_16_US
    );
    assert!(!mac.htc_he() && !mac.twt_requester() && !mac.trs() && !mac.bsr());
    assert!(!mac.om_control() && !mac.ack_enabled_aggregation() && !mac.broadcast_twt());
    // An access point reading the element finds the 16 us padding.
    let operation = [255, 7, 36, 0x04, 0x00, 0x00, 0x01, 0xfc, 0xff];
    assert_eq!(
        parse_he_peer_state(&HE20, &operation)
            .unwrap()
            .packet_padding_eight_us,
        2
    );
}

#[test]
fn no_extended_capabilities_go_beside_he20() {
    assert_eq!(HE20_EXTENDED, None);
}

#[test]
fn the_ht_capabilities_beside_he20_are_20_mhz_with_sm_power_save_disabled() {
    let [id, length, info0, info1, ampdu, mcs0, ..] = HE20_HT;
    assert_eq!((id, length), (45, 26));
    let info = HtCapabilityInfo::from_bytes([info0, info1]);
    assert_eq!(
        info.sm_power_save(),
        HtCapabilityInfo::SM_POWER_SAVE_DISABLED
    );
    // The 20 MHz short guard interval the encoder takes from the geometry,
    // and nothing else: no 40 MHz width, no STBC, no DSSS/CCK at 40 MHz.
    assert_eq!(u16::from_le_bytes([info0, info1]) & !0x000c, 0x0020);
    assert_eq!(ampdu, 0x17);
    assert_eq!(mcs0, 0xff);
}
