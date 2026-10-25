//! The HE20 peer and association leaves over host memory.

use super::{MacHe20PeerConfig, MacHe20PeerError};
use crate::ieee80211::registers::{
    HostWords, MacAssociationId, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacMinimumMpduStartSpacing, element,
};

fn config(extended_range_single_user_disabled: bool) -> MacHe20PeerConfig {
    MacHe20PeerConfig {
        packet_padding_duration: MacHePacketPaddingDuration::new(16).expect("in range"),
        default_packet_extension_duration: MacHeDefaultPacketExtensionDuration::new(4)
            .expect("in range"),
        bss_color: MacHeBssColor::new(0x2a).expect("in range"),
        bss_color_enabled: true,
        partial_bss_color: false,
        extended_range_single_user_disabled,
    }
}

#[test]
fn a_finite_rts_threshold_fails_closed_before_any_write() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    assert_eq!(
        after
            .owners()
            .registers
            .program_he20_peer(config(false), Some(512)),
        Err(MacHe20PeerError::UnsupportedRtsThreshold)
    );
    assert!(before.changes(&after).is_empty());
}

#[test]
fn a_peer_publishes_its_color_padding_and_rts_disable() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .program_he20_peer(config(false), None)
        .expect("no finite threshold");
    let mac = &owners.registers.mac;
    let multi = mac.wifi_mac_he_init_suffix.read_multi_bssid_control();
    assert!(multi.he_bssid_enable() && multi.bss_color_enable());
    assert!(!multi.partial_bss_color_enable());
    assert_eq!(multi.bss_color().get(), 0x2a);
    assert_eq!(multi.bssid_select().get(), 0);
    let power_save = mac.wifi_mac_rx_power_save.read_rx_power_save_control();
    assert!(power_save.intra_ppdu_ps_enable() && power_save.intra_ps_check_bss_color_enable());
    assert!(
        mac.wifi_mac_he_init_prefix
            .read_rx_field_control()
            .color_bitmap_clear()
    );
    assert_eq!(
        mac.wifi_mac_he_init_suffix
            .read_he_default_control()
            .default_pe_duration()
            .get(),
        4
    );
    let padding = mac.wifi_mac_he_init_suffix.read_he_packet_padding();
    for duration in [
        padding.bpsk_duration(),
        padding.qpsk_duration(),
        padding.qam16_duration(),
        padding.qam64_duration(),
        padding.qam256_duration(),
    ] {
        assert_eq!(duration.get(), 16);
    }
    for bank in 0..4 {
        let rts = mac
            .wifi_mac_tx_queue_vector
            .read_he_rts_control(element(bank));
        assert!(rts.he_rts_disabled() && !rts.rts_threshold_valid());
    }
    let suffix = &mac.wifi_mac_he_init_suffix;
    assert!(suffix.read_ersu_and_vht_control().auto_ack_allow_ersu());
    let ack = suffix.read_ersu_ack_rate();
    assert_eq!(
        [ack.rate_0(), ack.rate_1(), ack.rate_2(), ack.rate_3()],
        [0x80; 4]
    );
}

#[test]
fn a_disabled_extended_range_keeps_the_ack_rates() {
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    owners
        .registers
        .program_he20_peer(config(true), None)
        .expect("no finite threshold");
    let suffix = &owners.registers.mac.wifi_mac_he_init_suffix;
    assert!(!suffix.read_ersu_and_vht_control().auto_ack_allow_ersu());
    assert_eq!(suffix.read_ersu_ack_rate().rate_0(), 0xff);
}

#[test]
fn a_disabled_color_is_published_as_zero() {
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    let mut peer = config(false);
    peer.bss_color_enabled = false;
    owners
        .registers
        .program_he20_peer(peer, None)
        .expect("no finite threshold");
    let multi = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_multi_bssid_control();
    assert_eq!(multi.bss_color().get(), 0);
    assert!(!multi.bss_color_enable());
}

#[test]
fn an_association_publishes_its_identifier_and_broadcast_ru() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners.registers.program_he20_association(
        MacAssociationId::new(0x123).expect("in range"),
        MacMinimumMpduStartSpacing::new(5).expect("in range"),
        3,
    );
    let mac = &owners.registers.mac;
    let high = mac.wifi_mac_bssid_policy.read_bssid_high(element(0));
    assert_eq!(high.association_id().get(), 0x123);
    assert_eq!(high.minimum_mpdu_start_spacing().get(), 5);
    let low = mac.wifi_mac_he_init_suffix.read_broadcast_ru_low();
    assert_eq!(low.association_id().get(), 0x123);
    assert_eq!(low.value().get(), 3);
    assert!(low.enable());
    let ru_high = mac.wifi_mac_he_init_suffix.read_broadcast_ru_high();
    assert!(ru_high.low_enable() && ru_high.high_enable());
}
