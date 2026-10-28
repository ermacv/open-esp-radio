//! The words built through the generated layout are the words this crate
//! published with hand-written masks before it, among them the images the W1
//! SOURCE: HIL_W1_RX_WALKER_2026_10_18 in registers/evidence/hil-open.toml.

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
fn the_mpdu_length_bound_is_the_length_field_width() {
    let bound = u16::try_from(MAX_ENCODED_MPDU_LEN).unwrap_or(u16::MAX);
    assert!(rx(0).with_length(bound).is_some());
    assert!(rx(0).with_length(bound + 1).is_none());
}

#[test]
fn a_cursor_names_a_ring_descriptor_by_its_low_address_bits() {
    // A ring base above the 20 cursor bits: only the low bits are held.
    let base = 0x2f0c_0000_u32 + 0x1230;
    let low = |index: u32| {
        (base + index * DESCRIPTOR_BYTES)
            & crate::ieee80211::registers::WifiDescriptorAddressLow::MAX
    };
    assert_eq!(cursor_ring_index(low(0), base, 4), Some(0));
    assert_eq!(cursor_ring_index(low(3), base, 4), Some(3));
    assert_eq!(cursor_ring_index(low(4), base, 4), None);
    assert_eq!(cursor_ring_index(low(1) + 1, base, 4), None);
}

#[test]
fn a_descriptor_address_names_a_ring_index_only_inside_the_ring() {
    let base = 0x2f0c_1230_u32;
    assert_eq!(
        descriptor_ring_index(base + 2 * DESCRIPTOR_BYTES, base, 4),
        Some(2)
    );
    assert_eq!(
        descriptor_ring_index(base + 4 * DESCRIPTOR_BYTES, base, 4),
        None
    );
    assert_eq!(
        descriptor_ring_index(base - DESCRIPTOR_BYTES, base, 4),
        None
    );
    assert_eq!(descriptor_ring_index(base + 1, base, 4), None);
}
