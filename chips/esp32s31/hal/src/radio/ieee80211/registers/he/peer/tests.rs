//! The HE20 association's installation and removal over host memory.

use super::{MacHe20Association, MacHe20PeerConfig};
use crate::radio::ieee80211::registers::{
    HostWords, MacAssociationId, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacMinimumMpduStartSpacing, element,
};

fn peer(extended_range_single_user_disabled: bool) -> MacHe20PeerConfig {
    MacHe20PeerConfig {
        packet_padding_duration: MacHePacketPaddingDuration::new(16).expect("in range"),
        default_packet_extension_duration: MacHeDefaultPacketExtensionDuration::new(4)
            .expect("in range"),
        max_co_hosted_bssid_indicator: None,
        extended_range_single_user_disabled,
    }
}

fn color() -> MacHeBssColor {
    MacHeBssColor::new(0x2a).expect("in range")
}

fn association(peer: MacHe20PeerConfig) -> MacHe20Association {
    MacHe20Association {
        peer,
        association_id: MacAssociationId::new(0x123).expect("in range"),
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing::new(5).expect("in range"),
    }
}

#[test]
fn a_connection_and_its_association_publish_the_bssid_color_padding_and_rts_disable() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners.registers.connect_he20_bss(color(), true, false);
    owners
        .registers
        .install_he20_association(association(peer(false)));
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
    let high = mac.wifi_mac_bssid_policy.read_bssid_high(element(0));
    assert_eq!(high.association_id().get(), 0x123);
    assert_eq!(high.minimum_mpdu_start_spacing().get(), 5);
    let low = suffix.read_broadcast_ru_low();
    assert_eq!(low.association_id().get(), 0x123);
    assert_eq!(low.value().get(), 0);
    assert!(low.enable());
    let ru_high = suffix.read_broadcast_ru_high();
    assert!(ru_high.low_enable() && ru_high.high_enable());
    assert_eq!(
        (ru_high.low_value().get(), ru_high.high_value().get()),
        (0, 0)
    );
    assert!(suffix.read_ersu_and_vht_control().bsr_enable());
}

#[test]
fn a_disabled_extended_range_keeps_the_ack_rates() {
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    owners
        .registers
        .install_he20_association(association(peer(true)));
    let suffix = &owners.registers.mac.wifi_mac_he_init_suffix;
    assert!(!suffix.read_ersu_and_vht_control().auto_ack_allow_ersu());
    assert_eq!(suffix.read_ersu_ack_rate().rate_0(), 0xff);
}

#[test]
fn a_disabled_color_is_published_as_zero() {
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    owners.registers.connect_he20_bss(color(), false, true);
    let multi = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_multi_bssid_control();
    assert_eq!(multi.bss_color().get(), 0);
    assert!(!multi.bss_color_enable());
    assert!(multi.partial_bss_color_enable());
    assert!(multi.he_bssid_enable());
}

#[test]
fn the_association_leaves_the_connection_s_color_and_bssid_matching() {
    // The color is the connection's, before authentication; the
    // association after the response writes neither it nor HE BSSID
    // matching (`ieee80211_parse_heopr` writes the color of a connected
    // station only).
    let mut connected = HostWords::filled(0);
    connected
        .owners()
        .registers
        .connect_he20_bss(color(), true, true);
    let mut associated = connected.clone();
    associated
        .owners()
        .registers
        .install_he20_association(association(peer(false)));
    let multi = |words: &mut HostWords| {
        let owners = words.owners();
        let multi = owners
            .registers
            .mac
            .wifi_mac_he_init_suffix
            .read_multi_bssid_control();
        (
            multi.he_bssid_enable(),
            multi.bss_color().get(),
            multi.bss_color_enable(),
            multi.partial_bss_color_enable(),
        )
    };
    assert_eq!(multi(&mut connected), multi(&mut associated));
}

#[test]
fn a_co_hosted_bss_publishes_its_mask_and_none_restores_the_cold_init_s() {
    for (indicator, mask) in [(0, 0xff), (3, 0xf8), (8, 0x00)] {
        let mut words = HostWords::filled(0);
        let mut owners = words.owners();
        let mut hosted = peer(false);
        hosted.max_co_hosted_bssid_indicator = Some(indicator);
        owners
            .registers
            .install_he20_association(association(hosted));
        let multi = owners
            .registers
            .mac
            .wifi_mac_he_init_suffix
            .read_multi_bssid_control();
        assert!(multi.co_hosted_enable(), "{indicator}");
        assert_eq!(multi.multi_bssid_mask(), mask, "{indicator}");
    }
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .install_he20_association(association(peer(false)));
    let multi = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_multi_bssid_control();
    assert!(!multi.co_hosted_enable());
    assert_eq!(multi.multi_bssid_mask(), 0xff);
}

#[test]
fn an_enabled_multiple_bssid_keeps_the_co_hosted_fields() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .modify_multi_bssid_control(|w| {
            w.set_multi_bssid_enable(true);
        });
    let mut hosted = peer(false);
    hosted.max_co_hosted_bssid_indicator = Some(3);
    owners
        .registers
        .install_he20_association(association(hosted));
    let multi = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_multi_bssid_control();
    assert!(!multi.co_hosted_enable());
    assert_eq!(multi.multi_bssid_mask(), 0);
}

#[test]
fn a_removal_ends_bssid_matching_and_the_color() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners.registers.connect_he20_bss(color(), true, true);
    owners
        .registers
        .install_he20_association(association(peer(false)));
    owners.registers.clear_he20_association();
    let mac = &owners.registers.mac;
    let multi = mac.wifi_mac_he_init_suffix.read_multi_bssid_control();
    assert!(!multi.he_bssid_enable());
    assert!(!multi.bss_color_enable() && !multi.partial_bss_color_enable());
    assert_eq!(multi.bss_color().get(), 0);
    assert!(
        mac.wifi_mac_he_init_prefix
            .read_rx_field_control()
            .color_bitmap_clear()
    );
}

#[test]
fn a_removal_touches_only_the_bssid_word_and_the_bitmap_clear() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    after.owners().registers.clear_he20_association();
    // From zero, only the bitmap-clear request changes a word.
    assert_eq!(before.changes(&after).len(), 1);
}

#[test]
fn a_reinstallation_after_a_removal_is_the_first_installation() {
    let mut once = HostWords::filled(0);
    {
        let mut owners = once.owners();
        owners.registers.connect_he20_bss(color(), true, false);
        owners
            .registers
            .install_he20_association(association(peer(false)));
    }
    let mut again = HostWords::filled(0);
    {
        let mut owners = again.owners();
        owners.registers.connect_he20_bss(color(), false, true);
        owners
            .registers
            .install_he20_association(association(peer(true)));
        owners.registers.clear_he20_association();
        owners.registers.connect_he20_bss(color(), true, false);
        owners
            .registers
            .install_he20_association(association(peer(false)));
    }
    // A connection, its association, the removal and another connection
    // and association end where one of each does: the removal leaves
    // nothing the next one misses.
    assert!(once.changes(&again).is_empty());
}
