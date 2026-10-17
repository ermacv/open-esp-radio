const TEST_HT_CAPABILITIES: crate::ht::HtLocalCapabilities =
    crate::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

use super::*;
use crate::protection::HtProtectionMode;

#[test]
fn ht_operation_carries_the_current_protection_byte() {
    let channel = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    let element = ht_operation_ie(
        channel,
        HtOperationProtection {
            mode: HtProtectionMode::NonHtMixed,
            non_greenfield_present: true,
        },
    );
    assert_eq!(element[4], 0x07);
    assert_eq!(
        HtProtectionMode::from_operation_ie(Some(&element)),
        HtProtectionMode::NonHtMixed
    );
}

#[test]
fn ht20_records_are_complete_and_bounded() {
    let channel = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    let capability = ht_capability_ie(TEST_HT_CAPABILITIES, channel);
    assert_eq!(&capability[..2], &[45, 26]);
    assert_eq!(u16::from_le_bytes([capability[2], capability[3]]), 0x102c);
    assert_eq!(capability[5], 0xff);
    assert_eq!(capability[17], 0x01);
    assert_eq!(capability[18], 0);
    assert_eq!(
        ht_operation_ie(channel, HtOperationProtection::default())[..4],
        [61, 22, 6, 0]
    );
    assert_eq!(
        ht_peer_capabilities(&capability),
        Some(HtPeerCapabilities {
            capability_info: 0x102c,
            ampdu_parameters: 0x03,
            rx_mcs_0_to_7: 0xff,
            rx_ht_duplicate_mcs32: false,
        })
    );
    let mut empty = [0_u8; 28];
    empty[..2].copy_from_slice(&[45, 26]);
    assert_eq!(ht_peer_capabilities(&empty), None);
}

#[test]
fn ht40_records_keep_width_geometry_and_peer_facts_coherent() {
    let above = crate::channel::Channel::ghz2_4(6, WifiChannelWidth::Mhz40Above).unwrap();
    let below = crate::channel::Channel::ghz2_4(6, WifiChannelWidth::Mhz40Below).unwrap();
    let capability = ht_capability_ie(TEST_HT_CAPABILITIES, above);
    assert_eq!(u16::from_le_bytes([capability[2], capability[3]]), 0x106e);
    assert_eq!(
        ht_operation_ie(above, HtOperationProtection::default())[..4],
        [61, 22, 6, 0x05]
    );
    assert_eq!(
        ht_operation_ie(below, HtOperationProtection::default())[..4],
        [61, 22, 6, 0x07]
    );
    let peer = ht_peer_capabilities(&capability).unwrap();
    assert!(peer.supports_40_mhz());
    assert!(peer.supports_short_guard_interval(WifiChannelWidth::Mhz40Above));
    assert_eq!(peer.highest_rx_mcs(), 7);
    assert!(!peer.supports_ht_duplicate_mcs32());
    assert_eq!(capability[HtDuplicateMcs32::CAPABILITY_IE_BYTE], 0);
    assert_eq!(capability[17], 0x01);
}

#[test]
fn peer_mcs32_is_still_parsed_without_local_advertisement() {
    let channel = crate::channel::Channel::ghz2_4(6, WifiChannelWidth::Mhz40Above).unwrap();
    let local = ht_capability_ie(TEST_HT_CAPABILITIES, channel);
    let mut peer_record = local;
    HtDuplicateMcs32::new().advertise_receive_only(&mut peer_record);

    assert_eq!(local[HtDuplicateMcs32::CAPABILITY_IE_BYTE], 0);
    assert_eq!(local[17], 0x01);
    let peer = ht_peer_capabilities(&peer_record).unwrap();
    assert!(peer.supports_ht_duplicate_mcs32());
    assert_eq!(peer.ht_duplicate_mcs32(), Some(HtDuplicateMcs32::new()));

    let mut malformed_ht20 = ht_capability_ie(
        TEST_HT_CAPABILITIES,
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap(),
    );
    HtDuplicateMcs32::new().advertise_receive_only(&mut malformed_ht20);
    let malformed_peer = ht_peer_capabilities(&malformed_ht20).unwrap();
    assert!(!malformed_peer.supports_ht_duplicate_mcs32());
    assert_eq!(malformed_peer.ht_duplicate_mcs32(), None);
}

#[test]
fn peer_specific_capability_uses_vendor_ampdu_negotiation() {
    let channel = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    let mut peer_record = ht_capability_ie(TEST_HT_CAPABILITIES, channel);
    peer_record[4] = 0x17;
    let peer = ht_peer_capabilities(&peer_record).unwrap();

    assert_eq!(ht_capability_ie(TEST_HT_CAPABILITIES, channel)[4], 0x03);
    assert_eq!(
        ht_capability_ie_for_peer(TEST_HT_CAPABILITIES, channel, Some(peer))[4],
        0x17
    );

    peer_record[4] = 0x15;
    let stricter_exponent = ht_peer_capabilities(&peer_record).unwrap();
    assert_eq!(
        ht_capability_ie_for_peer(TEST_HT_CAPABILITIES, channel, Some(stricter_exponent))[4],
        0x15
    );
}

#[test]
fn local_profile_cannot_override_channel_geometry() {
    for width in [
        WifiChannelWidth::Mhz20,
        WifiChannelWidth::Mhz40Above,
        WifiChannelWidth::Mhz40Below,
    ] {
        let channel = crate::channel::Channel::ghz2_4(6, width).unwrap();
        let expected = ht_capability_ie(TEST_HT_CAPABILITIES, channel);
        for supplied_geometry in 0u16..8 {
            let geometry_bits = (supplied_geometry & 1) << 1
                | (supplied_geometry & 2) << 4
                | (supplied_geometry & 4) << 4;
            let profile = HtLocalCapabilities::new(0x100c | geometry_bits, 0x03, 0xff, 0x01);
            assert_eq!(ht_capability_ie(profile, channel), expected);
        }
    }
}
