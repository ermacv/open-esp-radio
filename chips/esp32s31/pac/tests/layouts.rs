#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
//! The generated memory layouts as values and cells: the builders and
//! getters agree, a value wider than its field is refused, cells keep what
//! they were given, and typed addresses round-trip. Field positions are the
//! generator's to prove (`tools/registers/generate`), not repeated here.

use oer_chips_esp32s31_pac::layouts::{
    DmaRef, DmaWindowAddress, WifiRxDmaDescriptor, WifiRxDmaDescriptorControl, WifiTxDmaDescriptor,
    WifiTxDmaDescriptorControl,
};

#[test]
fn an_rx_control_word_reads_back_what_its_builders_set() {
    let armed = WifiRxDmaDescriptorControl::ZERO
        .with_size(1664)
        .and_then(|value| value.with_length(1664))
        .expect("1664 fits both fields")
        .with_owner(true);
    assert!(armed.owner());
    assert!(!armed.rx_done());
    assert!(!armed.sosf());
    assert_eq!((armed.size(), armed.length()), (1664, 1664));

    let done = armed.with_rx_done(true).with_length(268).expect("268 fits");
    assert!(done.rx_done() && done.owner());
    assert_eq!((done.size(), done.length()), (1664, 268));
    // Clearing one bit leaves the others.
    let detached = done.with_owner(false);
    assert!(!detached.owner() && detached.rx_done());
    assert_eq!(detached.with_owner(true), done);
}

#[test]
fn a_value_that_does_not_fit_its_field_is_refused() {
    assert_eq!(WifiRxDmaDescriptorControl::ZERO.with_size(0x4000), None);
    assert_eq!(WifiTxDmaDescriptorControl::ZERO.with_length(u16::MAX), None);
}

#[test]
fn descriptor_cells_hold_what_was_written() {
    // SAFETY: host descriptors no hardware reads; the addresses only
    // round-trip through the cells.
    #[allow(unsafe_code, reason = "host-only typed addresses")]
    let (buffer, next) = unsafe {
        let next = DmaWindowAddress::new_unchecked(0x2f00_2000);
        (
            DmaWindowAddress::new_unchecked(0x2f00_1000),
            DmaRef::<WifiRxDmaDescriptor>::new_unchecked(next),
        )
    };
    let rx = WifiRxDmaDescriptor::zeroed();
    assert_eq!(rx.control().read(), WifiRxDmaDescriptorControl::ZERO);
    let value = WifiRxDmaDescriptorControl::ZERO.with_owner(true);
    rx.control().write(value);
    rx.buffer().write(rx.buffer().read().with_address(buffer));
    rx.next().write(rx.next().read().with_address(Some(next)));
    assert_eq!(rx.control().read(), value);
    assert_eq!(rx.buffer().read().address(), buffer.get());
    assert_eq!(rx.next().read().address(), next.address().get());
    // No link is a zero word.
    rx.next().write(rx.next().read().with_address(None));
    assert_eq!(rx.next().read().address(), 0);

    let tx = WifiTxDmaDescriptor::zeroed();
    tx.control().write(
        WifiTxDmaDescriptorControl::ZERO
            .with_tx_owned(true)
            .with_tx_last(true),
    );
    assert!(tx.control().read().tx_owned() && tx.control().read().tx_last());
    let zero: WifiTxDmaDescriptor = bytemuck::Zeroable::zeroed();
    assert_eq!(zero.control().read(), WifiTxDmaDescriptorControl::ZERO);
}

#[test]
fn a_reference_keeps_its_address() {
    // SAFETY: as above.
    #[allow(unsafe_code, reason = "host-only typed addresses")]
    let reference = unsafe {
        DmaRef::<WifiTxDmaDescriptor>::new_unchecked(DmaWindowAddress::new_unchecked(0x2f00_4000))
    };
    assert_eq!(reference.address().get(), 0x2f00_4000);
    assert_eq!(reference.address().get(), 0x2f00_4000);
    assert_eq!(reference, reference);
}

#[test]
fn both_descriptors_have_the_same_geometry() {
    assert_eq!(WifiRxDmaDescriptor::SIZE, WifiTxDmaDescriptor::SIZE);
    assert_eq!(WifiRxDmaDescriptor::ALIGN, WifiTxDmaDescriptor::ALIGN);
    assert_eq!(
        WifiRxDmaDescriptor::PLACEMENT,
        WifiTxDmaDescriptor::PLACEMENT
    );
}

#[test]
fn the_tx_metadata_entry_sets_only_the_field_it_names() {
    use oer_chips_esp32s31_pac::layouts::{WifiTxMetadataEntry, WifiTxMetadataEntryMut};

    let mut bytes = [0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(bytes.len(), WifiTxMetadataEntry::SIZE);
    let mut entry = WifiTxMetadataEntryMut::new(&mut bytes);
    let control = entry.control().with_single(true);
    entry.set_control(control);
    assert!(entry.view().control().single());
    assert_eq!(entry.view().control().mpdu_len(), 0x1c);
    assert_eq!(bytes, [0x1c, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);

    let mut entry = WifiTxMetadataEntryMut::new(&mut bytes);
    let control = entry.control().with_single(false);
    entry.set_control(control);
    let delimiters = entry.delimiters().with_htc(true);
    entry.set_delimiters(delimiters);
    assert_eq!(bytes, [0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);

    let entry = WifiTxMetadataEntryMut::new(&mut bytes);
    assert!(entry.view().delimiters().htc());
    assert_eq!(entry.view().delimiters().deli_num(), 0);
}
