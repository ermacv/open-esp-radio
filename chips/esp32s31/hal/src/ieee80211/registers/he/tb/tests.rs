//! The HE trigger-based leaves over host memory.

use crate::ieee80211::registers::HostWords;

#[test]
fn the_uplink_multi_user_flags_are_two_independent_edges() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .set_he_uplink_multi_user_disable(true, false);
    let control = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_he_default_control();
    assert!(control.ul_mu_disable() && !control.ul_mu_data_disable());
    owners
        .registers
        .set_he_uplink_multi_user_disable(false, true);
    let control = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_he_default_control();
    assert!(!control.ul_mu_disable() && control.ul_mu_data_disable());
}

#[test]
fn the_statistics_read_every_counter_without_a_write() {
    // Equal halves make each sixteen-bit counter read the same value.
    let before = HostWords::filled(0x0003_0003);
    let mut after = before.clone();
    let statistics = after.owners().registers.he_trigger_based_statistics();
    assert_eq!(statistics.rx_trigger_count, 3);
    assert_eq!(statistics.transmission_count, 3);
    assert_eq!(statistics.qos_null_count, 3);
    let _ = after.owners().registers.he_trigger_based_tx_diagnostics();
    assert!(before.changes(&after).is_empty());
}

#[test]
fn the_tx_diagnostics_read_every_field_whole() {
    let mut words = HostWords::filled(u32::MAX);
    let diagnostics = words.owners().registers.he_trigger_based_tx_diagnostics();
    assert_eq!(diagnostics.tx_time, u16::MAX);
    assert_eq!(diagnostics.symbol_count, 0x3ff);
    assert_eq!(diagnostics.pre_fec_padding_phy, 0x3f);
    assert_eq!(diagnostics.psdu_length, 0x1_ffff);
    assert_eq!(diagnostics.association_id, 0x1fff);
    assert_eq!(diagnostics.ru_allocation, 0x7f);
    assert_eq!(diagnostics.tid_limit, 7);
}
