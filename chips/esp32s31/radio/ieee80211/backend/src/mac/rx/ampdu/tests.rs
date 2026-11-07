use super::*;

#[test]
fn a_block_ack_request_names_its_session_regardless_of_policy_bits() {
    let local = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut raw = [0_u8; PUBLIC_HEADER_SIZE + 24];
    let frame = &mut raw[PUBLIC_HEADER_SIZE..];
    frame[..2].copy_from_slice(&[0x84, 0x00]);
    frame[4..10].copy_from_slice(&local);
    frame[10..16].copy_from_slice(&peer);
    // TID 5 with the No-Ack policy and Basic variant: not validated.
    frame[16..18].copy_from_slice(&(5_u16 << 12 | 1).to_le_bytes());
    frame[18..20].copy_from_slice(&(0x123_u16 << 4).to_le_bytes());
    let expected = RxBlockAckRequestKey {
        peer,
        tid: 5,
        starting_sequence: SequenceNumber::new(0x123).unwrap(),
    };
    assert_eq!(
        rx_block_ack_request_key(&raw, local, Some(peer)),
        Some(expected)
    );
    assert_eq!(rx_block_ack_request_key(&raw, local, None), Some(expected));
    assert_eq!(rx_block_ack_request_key(&raw, local, Some(local)), None);
    assert_eq!(rx_block_ack_request_key(&raw, peer, None), None);

    let mut high_tid = raw;
    high_tid[PUBLIC_HEADER_SIZE + 17] = 0x80;
    assert_eq!(rx_block_ack_request_key(&high_tid, local, None), None);
    let mut block_ack = raw;
    block_ack[PUBLIC_HEADER_SIZE] = 0x94;
    assert_eq!(rx_block_ack_request_key(&block_ack, local, None), None);
    assert_eq!(
        rx_block_ack_request_key(&raw[..PUBLIC_HEADER_SIZE + 19], local, None),
        None
    );
}
