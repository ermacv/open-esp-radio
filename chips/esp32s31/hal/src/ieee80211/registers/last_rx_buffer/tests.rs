//! The last-RX-buffer table initialization over host memory.

use crate::ieee80211::registers::HostWords;

#[test]
fn the_table_takes_every_vendor_word_then_its_enable_edges() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    memory.owners().registers.initialize_mac_last_rx_buffer();

    // Eighteen table words, the table control word and the CSI control word.
    let changes = before.changes(&memory);
    let table = changes
        .iter()
        .filter(|(part, _, _)| *part == "wifi_mac_last_rx_buffer")
        .count();
    assert_eq!(table, 19);
    assert_eq!(changes.len(), 20);
    // The whole-word parameter images land whole, undeclared bits included.
    let mut images: std::vec::Vec<u32> = changes
        .iter()
        .filter(|(part, _, _)| *part == "wifi_mac_last_rx_buffer")
        .map(|(_, _, value)| *value)
        .collect();
    images.sort_unstable();
    for image in [
        0x0000_ffff,
        0xffff_ffff,
        0x0000_00ff,
        0x0002_3011,
        0x4300_4400,
    ] {
        assert!(images.contains(&image), "{image:08x} in {images:08x?}");
    }

    let owners = memory.owners();
    let mac = &owners.registers.mac;
    let control = mac.wifi_mac_last_rx_buffer.read_last_rx_buffer_control();
    assert_eq!(control.bits_8_13_opaque().get(), 0x3f);
    assert_eq!(control.bits_1_6_opaque().get(), 0x3f);
    assert!(
        mac.wifi_mac_rx_csi_control
            .read_rx_csi_control()
            .bit_27_opaque()
    );
}
