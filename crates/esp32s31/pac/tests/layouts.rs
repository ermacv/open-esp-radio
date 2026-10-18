//! The generated memory layouts: values built through the `with_*` builders
//! read back through the getters, cells keep what they were given, and the
//! byte decoders read the fields the hardware writes.

use oer_esp32s31_pac::layouts::{
    WifiRxControlPrefix, WifiRxControlTail, WifiRxDmaDescriptor, WifiRxDmaDescriptorControl,
    WifiTxDmaDescriptor, WifiTxDmaDescriptorControl,
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
fn every_word_image_is_a_value_and_keeps_unnamed_bits() {
    let image = WifiRxDmaDescriptorControl::from_bits(u32::MAX);
    assert_eq!(image.bits(), u32::MAX);
    let cleared = image
        .with_owner(false)
        .with_rx_done(false)
        .with_bit_29_opaque(false)
        .with_size(0)
        .and_then(|value| value.with_length(0))
        .expect("zero fits");
    // Bit 28 belongs to no field and stays set.
    assert_ne!(cleared.bits(), 0);
    assert_eq!(
        cleared.with_owner(true).bits() & cleared.bits(),
        cleared.bits()
    );
}

#[test]
fn descriptor_cells_hold_what_was_written() {
    let rx = WifiRxDmaDescriptor::zeroed();
    assert_eq!(rx.control().read(), WifiRxDmaDescriptorControl::ZERO);
    let value = WifiRxDmaDescriptorControl::ZERO.with_owner(true);
    rx.control().write(value);
    rx.buffer()
        .write(rx.buffer().read().with_address(0x2f00_1000));
    rx.next().write(rx.next().read().with_address(0x2f00_2000));
    assert_eq!(rx.control().read(), value);
    assert_eq!(rx.buffer().read().address(), 0x2f00_1000);
    assert_eq!(rx.next().read().address(), 0x2f00_2000);

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
fn both_descriptors_have_the_same_geometry() {
    assert_eq!(WifiRxDmaDescriptor::SIZE, WifiTxDmaDescriptor::SIZE);
    assert_eq!(WifiRxDmaDescriptor::ALIGN, WifiTxDmaDescriptor::ALIGN);
    assert_eq!(
        WifiRxDmaDescriptor::PLACEMENT,
        WifiTxDmaDescriptor::PLACEMENT
    );
    // The TX word's size and length are the RX word's.
    let rx = WifiRxDmaDescriptorControl::ZERO
        .with_size(512)
        .and_then(|value| value.with_length(123))
        .expect("fits");
    let tx = WifiTxDmaDescriptorControl::from_bits(rx.bits());
    assert_eq!((tx.size(), tx.length()), (512, 123));
}

#[test]
fn the_prefix_decoder_reads_the_public_rx_control_fields() {
    let mut bytes = [0_u8; WifiRxControlPrefix::SIZE];
    bytes[0] = (-47_i8) as u8;
    bytes[1] = 0xeb;
    bytes[4..8].copy_from_slice(&0x0040_5b4b_u32.to_le_bytes());
    bytes[8] = 3;
    bytes[9..11].copy_from_slice(&0x1234_u16.to_le_bytes());
    bytes[0x0c..0x10].copy_from_slice(&0x8765_4321_u32.to_le_bytes());
    bytes[0x1f] = 1;
    bytes[0x25] = 0x4f;
    bytes[0x26] = 0x40;
    bytes[0x27] = 0x05;
    bytes[0x2a] = 0xe0 | 0x15;
    bytes[0x2b] = 0x80 | 0x12;
    let prefix = WifiRxControlPrefix::new(&bytes);
    assert_eq!(prefix.rssi().value() as i8, -47);
    assert_eq!(prefix.rate().rate(), 11);
    assert_eq!(prefix.he_siga1().value(), 0x0040_5b4b);
    assert_eq!(prefix.rxend_state().value(), 3);
    assert_eq!(prefix.he_siga2().value(), 0x1234);
    assert_eq!(prefix.timestamp().value(), 0x8765_4321);
    assert!(prefix.single_mpdu().single_mpdu());
    assert_eq!(prefix.bb_format().bb_format(), 4);
    assert_eq!(prefix.channel_estimate().length(), 0x140);
    assert!(prefix.channel_estimate().valid());
    let sig_b = prefix.he_sig_b();
    assert_eq!(sig_b.length_bits(), 7);
    assert_eq!(sig_b.length_bytes(), 0x12);
    assert!(sig_b.present());
    assert_eq!(sig_b.selected_user(), 0x15 << 16);
}

#[test]
fn the_tail_decoder_reads_the_lengths_and_the_state() {
    let mut bytes = [0_u8; WifiRxControlTail::SIZE];
    bytes[0..4].copy_from_slice(&(0xc000_0000_u32 | (32 << 16) | 28 | 0xc000).to_le_bytes());
    bytes[4] = 0xf5;
    bytes[5] = 0x7a;
    let tail = WifiRxControlTail::new(&bytes);
    assert_eq!(tail.signal_length().length(), 28);
    assert_eq!(tail.dump_length().length(), 32);
    assert_eq!(tail.rx_state().value(), 0xf5);
    assert_eq!(tail.byte_0005_opaque().bits_0_7_opaque(), 0x7a);
}
