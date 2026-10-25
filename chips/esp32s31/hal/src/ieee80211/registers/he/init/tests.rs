//! The HE init prefix over host memory: what it sets, what it keeps.

use std::vec;

use oer_esp32s31_pac::{instances, wifi_bb::PhyAgcOracleWifiBb};

use crate::ieee80211::registers::HostWords;

/// Run the prefix over memory filled with `fill`; return the memory before
/// and after and the AGC word's memory before and after.
fn run(fill: u32) -> (HostWords, HostWords, std::vec::Vec<u32>, std::vec::Vec<u32>) {
    let before = HostWords::filled(fill);
    let mut after = before.clone();
    let agc_before = vec![fill; instances::wifi_bb::SIZE.div_ceil(4)];
    let mut agc_after = agc_before.clone();
    {
        let agc = PhyAgcOracleWifiBb::host(&mut agc_after).expect("the part's host words");
        let mut owners = after.owners();
        owners.registers.initialize_mac_he_prefix(&agc);
    }
    (before, after, agc_before, agc_after)
}

#[test]
fn the_prefix_from_zero_sets_every_vendor_field() {
    let (before, mut after, agc_before, agc_after) = run(0);
    assert_eq!(agc_before, agc_after, "the AGC word is only read");
    let changes = before.changes(&after);
    assert!(
        changes.iter().all(
            |(part, _, _)| ["wifi_mac_he_init_prefix", "wifi_mac_rx_bssid_list"].contains(part)
        )
    );
    // Eight prefix words and the BSSID-position word; the TB clears and the
    // UL-MU clears store the zero already there.
    assert_eq!(changes.len(), 9);

    let owners = after.owners();
    let prefix = &owners.registers.mac.wifi_mac_he_init_prefix;
    let timing = prefix.read_bf_timing_control();
    assert!(timing.he_beam_hw_sequence_enable() && timing.bit_25_opaque());
    assert!(!timing.bf_memory_write_enable() && !timing.non_tb_beam_ru_select());
    assert_eq!(timing.he_beam_ndp_time(), 0x71);
    assert_eq!(timing.rx_bfrp_timeout_ms().get(), 0x10);
    assert_eq!(timing.he_beam_hw_sequence_select().get(), 5);
    assert!(prefix.read_bf_enable().bit_31_opaque());
    assert_eq!(
        prefix.read_bf_vector_control().bits_12_23_opaque().get(),
        0x801
    );
    assert_eq!(prefix.read_bf_high_image().bits_20_31_opaque().get(), 0x690);
    let mode = prefix.read_bf_mode_control();
    assert!(mode.high_selector_parity() && !mode.high_selector_class());
    let report = prefix.read_bf_report_rate();
    for (rate, signal_mode, dcm, ersu) in [
        (
            report.qam16_rate(),
            report.qam16_signal_mode(),
            report.qam16_dcm(),
            report.qam16_ersu(),
        ),
        (
            report.qpsk_rate(),
            report.qpsk_signal_mode(),
            report.qpsk_dcm(),
            report.qpsk_ersu(),
        ),
        (
            report.bpsk_rate(),
            report.bpsk_signal_mode(),
            report.bpsk_dcm(),
            report.bpsk_ersu(),
        ),
    ] {
        assert_eq!(
            (rate.get(), signal_mode.get(), dcm, ersu),
            (0, 1, false, false)
        );
    }
    assert_eq!(prefix.read_rx_field_control().timeout_seconds().get(), 0x3c);
    assert!(prefix.read_parent_enable().bit_12_opaque());
    assert_eq!(
        owners
            .registers
            .mac
            .wifi_mac_rx_bssid_list
            .read_rx_bssid_list_control()
            .rx_control_9_bssid_position()
            .get(),
        2
    );
}

#[test]
fn the_prefix_from_ones_clears_its_bits_and_keeps_the_others() {
    let (_, mut after, _, _) = run(u32::MAX);
    let owners = after.owners();
    let registers = &owners.registers;
    let options = registers
        .mac
        .wifi_mac_he_init_suffix
        .read_he_default_control();
    assert!(!options.ul_mu_disable() && !options.ul_mu_data_disable());
    assert!(options.bsr_update_enable(), "the UL-MU edges keep the rest");
    let prefix = &registers.mac.wifi_mac_he_init_prefix;
    let timing = prefix.read_bf_timing_control();
    assert!(!timing.non_tb_beam_ru_select() && !timing.bit_23_opaque());
    assert!(!timing.bf_memory_write_enable());
    assert!(timing.he_beam_enable() && timing.bit_24_opaque());
    assert_eq!(timing.bits_26_31_opaque().get(), 0x3f);
    assert!(!prefix.read_bf_mode_control().high_selector_class());
    let tb = prefix.read_tb_tx_control();
    assert!(!tb.clear_14() && !tb.clear_15());
    assert_eq!(tb.bits_16_31_opaque(), 0xffff);
    let report = prefix.read_bf_report_rate();
    assert!(!report.qam16_dcm() && !report.bpsk_ersu());
    assert_eq!(report.bits_27_31_opaque().get(), 0x1f);
}
