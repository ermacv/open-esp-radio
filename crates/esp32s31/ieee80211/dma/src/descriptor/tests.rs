//! The words built through the generated layout are the words this crate
//! published with hand-written masks before it, among them the images the W1
//! stand runs read (`knowledge/esp32s31/wifi/rx-walker.md`).

use super::*;

fn rx(bits: u32) -> WifiRxDmaDescriptorControl {
    WifiRxDmaDescriptorControl::from_bits(bits)
}

#[test]
fn an_armed_rx_word_is_the_stand_s_armed_image() {
    // Size = length = 1664, owner set, bits 30 and 29 clear.
    assert_eq!(
        rx_armed_word(1664).map(|word0| word0.bits()),
        Some(0x81a0_0680)
    );
    assert_eq!(
        rx_armed_word(16).map(|word0| word0.bits()),
        Some(0x8004_0010)
    );
    assert_eq!(rx_armed_word(0), None);
    assert_eq!(rx_armed_word(0x4000), None);
    assert_eq!(
        rx_armed_word(0x3fff).map(|word0| word0.bits()),
        Some(0x8fff_ffff)
    );
}

#[test]
fn detaching_clears_only_the_owner_bit() {
    // The stand's LAST image at exhaustion and its detached form.
    assert_eq!(rx_detached_word(rx(0xc043_0680)).bits(), 0x4043_0680);
    assert_eq!(rx_detached_word(rx(0x4043_0680)).bits(), 0x4043_0680);
}

#[test]
fn rearming_restores_the_armed_image_and_keeps_unrelated_bits() {
    assert_eq!(
        rx_rearm_word(rx(0xc043_0680)).map(|word0| word0.bits()),
        Some(0x81a0_0680)
    );
    assert_eq!(
        rx_rearm_word(rx(0x4043_0680)).map(|word0| word0.bits()),
        Some(0x81a0_0680)
    );
    // Bit 29 is cleared, bit 28 (no field) kept.
    assert_eq!(
        rx_rearm_word(rx(0xf043_0680)).map(|word0| word0.bits()),
        Some(0x91a0_0680)
    );
    assert_eq!(rx_rearm_word(rx(0xc043_0000)), None);
}

#[test]
fn a_completed_word_carries_the_received_length_and_the_terminal_bit() {
    let armed = rx_armed_word(1664).expect("fits");
    assert_eq!(
        rx_completed_word(armed, 268, true).map(|word0| word0.bits()),
        Some(0xc043_0680)
    );
    assert_eq!(
        rx_completed_word(armed, 268, false).map(|word0| word0.bits()),
        Some(0x8043_0680)
    );
    assert_eq!(rx_completed_word(armed, 0x4000, true), None);
}

#[test]
fn a_tx_word_is_size_length_last_and_owned() {
    assert_eq!(
        tx_owned_word(512, 123).map(|word0| word0.bits()),
        Some(0xc01e_c200)
    );
    assert_eq!(
        tx_owned_word(1664, 1664).map(|word0| word0.bits()),
        Some(0xc1a0_0680)
    );
    assert_eq!(tx_owned_word(64, 65), None);
    assert_eq!(tx_owned_word(64, 0), None);
    assert_eq!(tx_owned_word(0x4000, 1), None);
    assert_eq!(
        tx_owned_word(512, 123).map(|word0| word0.with_tx_last(false).bits()),
        Some(0x801e_c200)
    );
}

#[test]
fn the_dma_window_bounds_descriptors_and_buffers() {
    assert!(descriptor_address_valid(DMA_WINDOW.start));
    assert!(!descriptor_address_valid(DMA_WINDOW.start + 2));
    assert!(!descriptor_address_valid(DMA_WINDOW.end - 8));
    assert!(dma_range_valid(DMA_WINDOW.end - 8, 8));
    assert!(!dma_range_valid(DMA_WINDOW.end - 8, 9));
    assert!(!dma_range_valid(DMA_WINDOW.start - 4, 4));
    assert!(!dma_range_valid(DMA_WINDOW.start, 0));
}

#[test]
fn the_mpdu_length_bound_is_the_length_field_width() {
    let bound = u16::try_from(MAX_ENCODED_MPDU_LEN).unwrap_or(u16::MAX);
    assert!(rx(0).with_length(bound).is_some());
    assert!(rx(0).with_length(bound + 1).is_none());
}
