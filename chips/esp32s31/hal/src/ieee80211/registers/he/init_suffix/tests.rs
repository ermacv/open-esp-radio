//! The HE init suffix and its sibling leaves over host memory.

use std::vec;

use oer_esp32s31_pac::{instances, wifi_mac::PhyFrequencyChannelOracleWifiMac};

use super::MacHeTxMpduLengthLink;
use crate::ieee80211::registers::{HostWords, element};

/// Run the suffix over memory filled with `fill`; return the Wi-Fi memory
/// after it and the channel word's memory after it.
fn run(fill: u32) -> (HostWords, std::vec::Vec<u32>) {
    let mut words = HostWords::filled(fill);
    let mut channel = vec![fill; instances::wifi_mac::SIZE.div_ceil(4)];
    {
        let mut part =
            PhyFrequencyChannelOracleWifiMac::host(&mut channel).expect("the part's host words");
        let mut owners = words.owners();
        owners.registers.initialize_mac_he_suffix(&mut part);
    }
    (words, channel)
}

#[test]
fn the_suffix_from_zero_publishes_the_vendor_baseline() {
    let (mut words, mut channel) = run(0);
    let part = PhyFrequencyChannelOracleWifiMac::host(&mut channel).expect("the part's host words");
    let offsets = part.read_channel_tx_offset_control();
    assert_eq!(offsets.minimum_power_low().get(), 5);
    assert_eq!(offsets.minimum_power_high().get(), 3);
    assert!(offsets.bit_17_opaque());

    let owners = words.owners();
    let suffix = &owners.registers.mac.wifi_mac_he_init_suffix;
    let ack = suffix.read_ersu_ack_rate();
    assert_eq!(
        [ack.rate_0(), ack.rate_1(), ack.rate_2(), ack.rate_3()],
        [0x80; 4]
    );
    let control = suffix.read_ersu_and_vht_control();
    assert!(control.auto_ack_allow_ersu() && !control.vht_bw_signaling_rts());
    assert_eq!(
        suffix.read_he_default_control().mpdu_length_offset().get(),
        0x17c
    );
    assert!(
        suffix
            .read_he_init_suffix_shared_enable_control()
            .bit_31_opaque()
    );
    let edges = suffix.read_he_init_suffix_feature_edges();
    assert!(edges.enable_0() && edges.enable_1());
    assert_eq!(
        suffix
            .read_tx_mode_control()
            .beacon_filter_frame_crc_state()
            .get(),
        1
    );
    let low = suffix.read_broadcast_ru_low();
    assert!(low.enable());
    assert_eq!(low.value().get(), 0x7fd);
    let high = suffix.read_broadcast_ru_high();
    assert!(high.low_enable() && high.high_enable());
    assert_eq!((high.low_value().get(), high.high_value().get()), (0, 0));
    let uora = suffix.read_uora_control();
    assert_eq!((uora.low_window().get(), uora.high_window().get()), (7, 31));
    // Both guarded leaves ran: the hosted mask is published.
    let multi = suffix.read_multi_bssid_control();
    assert_eq!(multi.multi_bssid_mask(), 0xff);
    assert!(!multi.multi_bssid_enable() && !multi.co_hosted_enable());
    assert!(
        owners
            .registers
            .mac
            .wifi_mac_txrx_prefix
            .read_mode_control()
            .bit_2_opaque()
    );
}

#[test]
fn the_suffix_from_ones_clears_the_link_table_cts_and_translation() {
    let (mut words, _) = run(u32::MAX);
    let owners = words.owners();
    let registers = &owners.registers;
    for index in 0..120_u8 {
        assert_eq!(
            registers.he_tx_mpdu_length_link(index),
            Some(MacHeTxMpduLengthLink {
                mpdu_length: 0,
                next_link: 0,
            })
        );
    }
    for physical in 0..4 {
        let protection = registers
            .mac
            .wifi_mac_tx_queue_control
            .read_protection(element(physical));
        assert!(!protection.software_cts() && protection.software_rts());
    }
    let suffix = &registers.mac.wifi_mac_he_init_suffix;
    for physical in 0..8 {
        let queue = suffix.read_queue_control(element(physical));
        assert!(!queue.qos_null_to_translated_bss() && queue.trigger_based_enable());
    }
    let multi = suffix.read_multi_bssid_control();
    assert!(!multi.multi_bssid_enable() && !multi.co_hosted_enable());
    assert_eq!(multi.bssid_byte_5(), 0);
    assert!(multi.bss_color_enable(), "the clear leaves keep the color");
    let high = suffix.read_multi_bssid_high();
    assert_eq!(
        (high.bits_16_23_opaque(), high.bits_24_31_opaque()),
        (0, 0xff)
    );
    assert!(!suffix.read_dump_complete_hesigb().enable());
}

#[test]
fn the_link_table_read_is_bounded() {
    let mut words = HostWords::filled(0);
    let owners = words.owners();
    assert!(owners.registers.he_tx_mpdu_length_link(119).is_some());
    assert_eq!(owners.registers.he_tx_mpdu_length_link(120), None);
}

#[test]
fn the_buffer_status_report_init_sets_its_eight_edges() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    after
        .owners()
        .registers
        .initialize_he_buffer_status_report();
    let changes = before.changes(&after);
    assert!(
        changes
            .iter()
            .all(|(part, _, _)| *part == "wifi_mac_he_init_suffix")
    );
    assert_eq!(changes.len(), 2);
    let owners = after.owners();
    let suffix = &owners.registers.mac.wifi_mac_he_init_suffix;
    let control = suffix.read_ersu_and_vht_control();
    assert!(control.bsr_enable() && control.ignore_valid_edca() && control.ignore_valid_tb());
    assert!(control.qos_data_update_bsr());
    assert!(!control.preac_no_resource_enable_bsr() && !control.ac_empty_single_bsr());
    assert_eq!(control.bsr_aci_high_comparison_method().get(), 1);
    assert!(suffix.read_he_default_control().bsr_update_enable());
}

#[test]
fn the_extended_range_disable_bit_is_inverted_into_the_ack_allowance() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .set_he_extended_range_single_user_disabled(false);
    let read = |owners: &crate::ieee80211::registers::HostOwners<'_>| {
        owners
            .registers
            .mac
            .wifi_mac_he_init_suffix
            .read_ersu_and_vht_control()
            .auto_ack_allow_ersu()
    };
    assert!(read(&owners));
    owners
        .registers
        .set_he_extended_range_single_user_disabled(true);
    assert!(!read(&owners));
}
