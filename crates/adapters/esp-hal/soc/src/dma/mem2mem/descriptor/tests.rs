use super::*;

#[test]
fn descriptor_payload_is_burst_aligned_and_below_hardware_limit() {
    assert_eq!(descriptor_payload_bytes(BurstSize::Bytes16), 4080);
    assert_eq!(descriptor_payload_bytes(BurstSize::Bytes32), 4064);
    assert_eq!(descriptor_payload_bytes(BurstSize::Bytes64), 4032);
}

#[test]
fn descriptor_count_covers_full_wifi_promotion_batch() {
    assert_eq!(required_descriptors(32 * 1536, BurstSize::Bytes32), 13);
    assert_eq!(required_descriptors(4064, BurstSize::Bytes32), 1);
    assert_eq!(required_descriptors(4065, BurstSize::Bytes32), 2);
}

#[test]
fn m2m_descriptor_publishes_size_and_length_on_both_chains() {
    let ordinary = descriptor_flags(64, false);
    assert_eq!(ordinary & 0x0fff, 64);
    assert_eq!((ordinary >> 12) & 0x0fff, 64);
    assert_eq!(ordinary & DESCRIPTOR_SUCCESS_EOF, 0);
    assert_ne!(ordinary & DESCRIPTOR_OWNER_DMA, 0);

    let terminal_tx = descriptor_flags(64, true);
    assert_ne!(terminal_tx & DESCRIPTOR_SUCCESS_EOF, 0);
}
