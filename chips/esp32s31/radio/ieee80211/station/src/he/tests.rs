//! The HE20 advertisement claims nothing optional, and the power elements
//! follow the PHY's target powers under the transmit ceiling.

use super::*;
use oer_chips_esp32s31_radio_phy::PhyConfig;
use oer_protocols_ieee80211_wire::he::{
    HeDcmConstellation, parse_he20_capabilities, parse_he20_peer_state,
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
    let capability = parse_he20_capabilities(&HE20).unwrap();
    assert!(capability.supports_bidirectional_mcs9());
    assert!(!capability.supports_ldpc_coding_in_payload());
    assert!(!capability.supports_one_ltf_800ns_gi());
    assert!(!capability.stbc_receive_under_80_mhz && !capability.stbc_transmit_under_80_mhz);
    assert_eq!(capability.dcm_receive, HeDcmConstellation::NotSupported);
    assert_eq!(capability.dcm_transmit, HeDcmConstellation::NotSupported);
    assert!(!capability.triggered_su_beamforming_feedback);
    assert!(!capability.triggered_cqi_feedback && !capability.non_triggered_cqi_feedback);
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
        parse_he20_peer_state(&HE20, &operation)
            .unwrap()
            .packet_padding_eight_us,
        2
    );
}

#[test]
fn the_extended_capabilities_beside_he20_claim_none() {
    let [id, length, capabilities @ ..] = HE20_EXTENDED;
    assert_eq!((id, length), (127, 12));
    assert_eq!(capabilities, [0; 12]);
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

/// The production PHY's power of each rate code under `ceiling` dBm.
fn production(ceiling: i8) -> impl Fn(u8) -> i8 {
    let targets = PhyConfig::production().tx_target_power_profile();
    move |rate| targets.pair(rate).primary.min(ceiling)
}

#[test]
fn the_power_elements_follow_the_production_targets() {
    // HE-MCS 0..9 at 20, 20, 20, 20, 19, 19, 18, 18, 16, 15: the indices of
    // the vendor run the archive recorded (open-esp-radio-rs `7abbf8eb6`).
    let power = he_power(-11, production(20)).unwrap();
    assert_eq!(power.power_capability.minimum_dbm(), -11);
    assert_eq!(power.power_capability.maximum_dbm(), 20);
    assert_eq!(
        power.ul_mu_power.relative_to_rate_16(),
        [0, 0, 0, 1, 1, 2, 2, 4, 5]
    );
}

#[test]
fn the_ceiling_bounds_every_rate() {
    let power = he_power(-11, production(15)).unwrap();
    assert_eq!(power.power_capability.maximum_dbm(), 15);
    assert_eq!(power.ul_mu_power.relative_to_rate_16(), [0; 9]);
}

#[test]
fn a_ceiling_below_the_minimum_gives_no_power_elements() {
    assert!(matches!(
        he_power(-11, production(-12)),
        Err(HePowerError::Capability(_))
    ));
    // A higher rate above HE-MCS 0 has no UL-MU difference.
    assert!(matches!(
        he_power(-11, |rate| if rate == 25 { 21 } else { 20 }),
        Err(HePowerError::UlMu(_))
    ));
}
