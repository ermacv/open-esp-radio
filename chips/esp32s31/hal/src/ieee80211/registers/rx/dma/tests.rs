//! The RX walker's register operations over host memory: what each writes
//! and what it keeps.

use crate::{ieee80211::registers::HostWords, memory::dma_address::model_reference};

/// A memory pattern with every opaque field set, so a kept bit shows.
const FILL: u32 = 0xa5a5_a5a5;

#[test]
fn the_buffer_prefix_publishes_its_geometry_and_the_window_prefix_only() {
    let before = HostWords::filled(FILL);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        owners.rx_dma.initialize_mac_rx_buffer_prefix();
        let dma = &owners.rx_dma.dma;
        assert_eq!(
            dma.read_word_0c68_opaque().bits_0_19_opaque().get(),
            0x000f_ffff
        );
        assert_eq!(dma.read_word_0c6c_opaque().bits_0_19_opaque().get(), 4);
        assert_eq!(dma.read_word_007c_opaque().rx_control_buffer_offset(), 0);
        assert_eq!(
            dma.read_word_007c_opaque().bit_28_opaque(),
            FILL & (1 << 28) != 0,
            "bit 28 kept"
        );
        assert_eq!(
            dma.read_rx_descriptor_high_window().address_high().get(),
            0x2f0
        );
    }
    // Four words change, all in the walker's part, and its control and
    // descriptor words keep their value.
    let changes = before.changes(&after);
    assert_eq!(changes.len(), 4);
    assert!(changes.iter().all(|(part, _, _)| *part == "rx_dma"));
}

#[test]
fn the_descriptor_base_takes_the_typed_reference_whole() {
    let mut words = HostWords::filled(FILL);
    let mut owners = words.owners();
    let head = model_reference(0x2f00_1240);
    owners.rx_dma.write_mac_rx_descriptor_base(head);
    assert_eq!(
        owners.rx_dma.dma.read_rx_descriptor_base().address(),
        head.bits()
    );
    assert_eq!(
        owners.rx_dma.mac_rx_dma_snapshot().descriptor_base,
        head.bits()
    );
}

#[test]
fn the_doorbell_sets_only_its_request_and_reads_back_pending() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    assert!(!owners.rx_dma.mac_rx_reload_pending());
    owners.rx_dma.request_mac_rx_descriptor_reload();
    // Host memory does not clear the doorbell as the hardware does.
    assert!(owners.rx_dma.mac_rx_reload_pending());
    let control = owners.rx_dma.dma.read_rx_control();
    assert!(!control.walker_enable());
    assert_eq!(control.bits_1_26_opaque().get(), 0);
}

#[test]
fn the_walker_enable_edges_keep_every_other_control_bit() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        assert!(owners.rx_dma.try_enable_mac_rx_walker());
        assert!(!owners.rx_dma.try_enable_mac_rx_walker(), "already enabled");
        assert!(owners.rx_dma.mac_rx_dma_snapshot().walker_enabled);
        owners.rx_dma.set_mac_rx_control_bit_27();
    }
    let control = after.owners().rx_dma.dma.read_rx_control();
    assert!(control.walker_enable() && control.bit_27_opaque());
    assert!(!control.append_descriptor_reload());
    assert_eq!(before.changes(&after).len(), 1);

    let mut owners = after.owners();
    assert!(owners.rx_dma.try_disable_mac_rx_walker());
    let control = owners.rx_dma.dma.read_rx_control();
    assert!(!control.walker_enable() && control.bit_27_opaque());
    owners.rx_dma.publish_mac_rx_walker_enable();
    owners.rx_dma.clear_mac_rx_walker_enable();
    assert!(!owners.rx_dma.mac_rx_walker_enabled());
}

#[test]
fn the_next_descriptor_observation_keeps_the_complete_word_zero_predicate() {
    let mut words = HostWords::filled(0);
    let owners = words.owners();
    let next = owners.rx_dma.mac_rx_next_descriptor();
    assert!(next.is_zero() && !next.has_unknown_upper_state());
    drop(owners);

    let mut words = HostWords::filled(u32::MAX);
    let owners = words.owners();
    let next = owners.rx_dma.mac_rx_next_descriptor();
    assert!(!next.is_zero() && next.has_unknown_upper_state());
    assert_eq!(next.address_low(), 0x000f_ffff);
    assert_eq!(owners.rx_dma.mac_rx_last_descriptor_low(), 0x000f_ffff);
}
