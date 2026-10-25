//! The SoftAP TSF lifecycle edges over host memory.

use oer_esp32s31_pac::modem_pwr::{softap_control, tsf_load_control, value_high, value_low};

use crate::ieee80211::registers::HostWords;

#[test]
fn a_reset_clears_the_load_words_and_restarts_the_domain() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.reset_and_start_softap_tsf();
    // The control word is cleared and set again, and the load request was
    // already set: only the two load words keep a change.
    assert_eq!(
        before.changes(&words),
        [
            ("wifi_mac_tsf_load", value_low::OFFSET, 0),
            ("wifi_mac_tsf_load", value_high::OFFSET, 0),
        ]
    );
}

#[test]
fn a_reset_from_zero_requests_the_softap_load_and_enables_both_bits() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words.owners().registers.reset_and_start_softap_tsf();
    let offsets: std::vec::Vec<_> = before
        .changes(&words)
        .into_iter()
        .map(|(part, offset, _)| (part, offset))
        .collect();
    assert_eq!(
        offsets,
        [
            ("wifi_mac_aux_tsf_control", softap_control::OFFSET),
            ("wifi_mac_tsf_load", tsf_load_control::OFFSET),
        ]
    );
    let owners = words.owners();
    let control = owners
        .registers
        .mac
        .wifi_mac_aux_tsf_control
        .read_softap_control();
    assert!(control.tsf_enable() && control.bit_30_opaque());
    assert_eq!(control.bits_0_29_opaque().get(), 0);
    let load = owners
        .registers
        .mac
        .wifi_mac_tsf_load
        .read_tsf_load_control();
    assert!(load.load_softap_tsf());
    assert!(!(load.load_station_tsf() || load.load_station_tbtt_target()));
}

#[test]
fn a_stop_clears_both_high_control_bits() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.stop_softap_tsf();
    let changes = before.changes(&words);
    assert_eq!(changes.len(), 1);
    let owners = words.owners();
    let control = owners
        .registers
        .mac
        .wifi_mac_aux_tsf_control
        .read_softap_control();
    assert!(!(control.tsf_enable() || control.bit_30_opaque()));
    assert_eq!(control.bits_0_29_opaque().get(), 0x3fff_ffff);
}
