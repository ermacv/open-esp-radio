//! The generated memory layouts as values and cells: the builders and
//! getters agree, a value wider than its field is refused, cells keep what
//! they were given, and typed addresses round-trip. Field positions are the
//! generator's to prove (`tools/lib/service/registers`), not repeated here.

use oer_esp32s31_pac::layouts::{
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
    assert!(!armed.bit_29_opaque());
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
    #[allow(
        unsafe_code,
        clippy::disallowed_methods,
        reason = "host-only typed addresses"
    )]
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
    assert_eq!(rx.next().read().address(), next.bits());
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
    #[allow(
        unsafe_code,
        clippy::disallowed_methods,
        reason = "host-only typed addresses"
    )]
    let reference = unsafe {
        DmaRef::<WifiTxDmaDescriptor>::new_unchecked(DmaWindowAddress::new_unchecked(0x2f00_4000))
    };
    assert_eq!(reference.address().get(), 0x2f00_4000);
    assert_eq!(reference.bits(), 0x2f00_4000);
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
