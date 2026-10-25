//! The cold antenna transaction over host memory.

use std::vec::Vec;

use crate::ieee80211::registers::{HostWords, element};

/// The parts an operation changed, one entry per changed word.
fn changed_parts(before: &HostWords, after: &HostWords) -> Vec<&'static str> {
    before
        .changes(after)
        .into_iter()
        .map(|(part, _, _)| part)
        .collect()
}

#[test]
fn antenna_init_from_zero_sets_each_bank_enable_and_the_common_bit() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    memory.owners().registers.initialize_mac_antenna();

    assert_eq!(
        changed_parts(&before, &memory),
        [
            "wifi_mac_antenna_init",
            "wifi_mac_antenna_init",
            "wifi_mac_antenna_init",
            "wifi_mac_antenna_init",
            "wifi_mac_antenna_init",
            "wifi_mac_tx_queue_vector",
            "wifi_mac_tx_queue_vector",
            "wifi_mac_tx_queue_vector",
            "wifi_mac_tx_queue_vector",
        ]
    );
    let owners = memory.owners();
    let mac = &owners.registers.mac;
    for bank in 0..4 {
        let vector = mac
            .wifi_mac_tx_queue_vector
            .read_length_control(element(bank));
        assert!(vector.tx_queue_antenna_enable());
        assert!(!vector.tx_queue_antenna_last() && !vector.tx_queue_antenna_value());
        assert!(!vector.txrx_queue_antenna_enable());
        let control = mac.wifi_mac_antenna_init.read_bank_control(element(bank));
        assert!(control.bit_5_opaque());
        assert!(!(control.bit_2_opaque() || control.bit_3_opaque() || control.bit_4_opaque()));
    }
    let common = mac.wifi_mac_antenna_init.read_common_control();
    assert!(common.bit_5_opaque() && !common.bit_2_opaque());
}

#[test]
fn antenna_init_from_ones_clears_only_the_vendor_bits() {
    let before = HostWords::filled(u32::MAX);
    let mut memory = before.clone();
    memory.owners().registers.initialize_mac_antenna();

    // Every touched word loses bits, none gains one: the cleared fields of
    // the eight banks and of the common word.
    let changes = before.changes(&memory);
    assert_eq!(changes.len(), 9);
    assert!(changes.iter().all(|(_, _, value)| *value != u32::MAX));
    let owners = memory.owners();
    let mac = &owners.registers.mac;
    for bank in 0..4 {
        let vector = mac
            .wifi_mac_tx_queue_vector
            .read_length_control(element(bank));
        assert!(vector.tx_queue_antenna_enable() && vector.txrx_queue_antenna_last());
        assert!(
            !(vector.txrx_queue_antenna_enable()
                || vector.tx_queue_antenna_last()
                || vector.tx_queue_antenna_value())
        );
        assert_eq!(vector.rts_rate(), u8::MAX);
        let control = mac.wifi_mac_antenna_init.read_bank_control(element(bank));
        assert!(control.bit_5_opaque());
        assert!(!(control.bit_2_opaque() || control.bit_3_opaque() || control.bit_4_opaque()));
        assert_eq!(control.bits_6_31_opaque().get(), (1 << 26) - 1);
    }
    let common = mac.wifi_mac_antenna_init.read_common_control();
    assert!(common.bit_5_opaque() && !common.bit_2_opaque());
}
