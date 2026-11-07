use super::*;

#[test]
fn station_and_access_point_share_one_public_reorder_classifier() {
    let local = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut raw = [0_u8; PUBLIC_HEADER_SIZE + 32];
    let frame = &mut raw[PUBLIC_HEADER_SIZE..];
    frame[..2].copy_from_slice(&0x4188_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&local);
    frame[10..16].copy_from_slice(&peer);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[24] = 5;
    let expected = RxBlockAckMpduKey {
        peer,
        tid: 5,
        sequence: SequenceNumber::new(0x123).unwrap(),
        retry: false,
    };

    assert_eq!(
        rx_block_ack_mpdu_key(&raw, local, Some(peer)),
        Some(expected)
    );
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), Some(expected));
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, Some([3; 6])), None);

    raw[PUBLIC_HEADER_SIZE + 22] |= 1;
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), None);

    raw[PUBLIC_HEADER_SIZE + 22] &= !1;
    raw[PUBLIC_HEADER_SIZE + 1] |= 0x04;
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), None);
}

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
