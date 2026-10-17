use super::*;

#[test]
fn ccmp_header_uses_the_recovered_48_bit_layout() {
    assert_eq!(
        ccmp_header(0x4433_2211, 0x8877_6655, 0x80),
        [0x11, 0x22, 0, 0xa0, 0x33, 0x44, 0x55, 0x66]
    );
}

#[test]
fn strict_header_round_trip_rejects_reserved_encodings() {
    let packet_number = CcmpPacketNumber::new(0x6655_4433_2211).unwrap();
    let header = CcmpHeader::new(packet_number, CcmpKeyId::new(2).unwrap());
    let encoded = header.encode();
    assert_eq!(encoded, [0x11, 0x22, 0, 0xa0, 0x33, 0x44, 0x55, 0x66]);
    assert_eq!(CcmpHeader::parse(encoded), Ok(header));

    let mut reserved_octet = encoded;
    reserved_octet[2] = 1;
    assert_eq!(
        CcmpHeader::parse(reserved_octet),
        Err(CcmpHeaderError::ReservedOctet)
    );
    let mut no_ext_iv = encoded;
    no_ext_iv[3] &= !0x20;
    assert_eq!(
        CcmpHeader::parse(no_ext_iv),
        Err(CcmpHeaderError::ExtIvMissing)
    );
    let mut reserved_key_bits = encoded;
    reserved_key_bits[3] |= 1;
    assert_eq!(
        CcmpHeader::parse(reserved_key_bits),
        Err(CcmpHeaderError::ReservedKeyBits)
    );
}

#[test]
fn receive_sequence_rejects_truncating_high_bits() {
    assert_eq!(
        CcmpPacketNumber::from_receive_sequence([1, 2, 3, 4, 5, 6, 0, 0]),
        Ok(CcmpPacketNumber::new(0x0605_0403_0201).unwrap())
    );
    assert_eq!(
        CcmpPacketNumber::from_receive_sequence([0, 0, 0, 0, 0, 0, 1, 0]),
        Err(CcmpReceiveSequenceError::HighBitsSet)
    );
}

#[test]
fn replay_admission_borrows_one_owner_and_cancel_does_not_advance_it() {
    let initial = CcmpPacketNumber::new(7).unwrap();
    let mut replay = CcmpRxReplayState::new(initial);
    let tid0 = CcmpReplayLane::Tid(0);
    let tid7 = CcmpReplayLane::Tid(7);
    let pn8 = CcmpPacketNumber::new(8).unwrap();
    let pn9 = CcmpPacketNumber::new(9).unwrap();
    let admission = replay.prepare(tid0, pn8).unwrap();
    assert_eq!(admission.lane(), tid0);
    assert_eq!(admission.packet_number(), pn8);
    admission.commit();
    assert_eq!(replay.highest(tid0), Some(pn8));
    {
        let _cancelled = replay.prepare(tid0, pn9).unwrap();
    }
    assert_eq!(replay.highest(tid0), Some(pn8));
    assert!(matches!(
        replay.prepare(tid0, pn8),
        Err(CcmpReplayError::Replayed { .. })
    ));
    replay.prepare(tid7, pn8).unwrap().commit();
    assert_eq!(replay.highest(tid7), Some(pn8));
    assert_eq!(replay.highest(CcmpReplayLane::NonQos), Some(initial));
    replay.commit_immediate(tid7, pn9).unwrap();
    assert_eq!(replay.highest(tid7), Some(pn9));
    assert_eq!(
        replay.commit_immediate(tid7, pn9),
        Err(CcmpReplayError::Replayed {
            packet_number: pn9,
            highest: pn9
        })
    );
    assert_eq!(replay.highest(tid0), Some(pn8));
    assert!(matches!(
        replay.prepare(CcmpReplayLane::Tid(16), pn9),
        Err(CcmpReplayError::InvalidTid)
    ));
    assert_eq!(
        replay.commit_immediate(CcmpReplayLane::Tid(16), pn9),
        Err(CcmpReplayError::InvalidTid)
    );
}

#[test]
fn transmit_packet_numbers_advance_by_their_step_from_a_new_key() {
    let mut ordinary = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE);
    assert_eq!(ordinary.last(), CcmpPacketNumber::ZERO);
    assert_eq!(ordinary.allocate(), Ok(CcmpPacketNumber::new(1).unwrap()));
    assert_eq!(ordinary.allocate(), Ok(CcmpPacketNumber::new(2).unwrap()));

    let mut stepped = CcmpTxPacketNumber::new(CcmpPacketNumberStep::new(3).unwrap());
    assert_eq!(
        stepped.next_header(CcmpKeyId::new(1).unwrap()),
        Ok(CcmpHeader::new(
            CcmpPacketNumber::new(3).unwrap(),
            CcmpKeyId::new(1).unwrap()
        )
        .encode())
    );
    assert_eq!(
        stepped.next_header(CcmpKeyId::PAIRWISE),
        Ok([6, 0, 0, 0x20, 0, 0, 0, 0])
    );
    assert_eq!(stepped.last(), CcmpPacketNumber::new(6).unwrap());
    assert!(CcmpPacketNumberStep::new(0).is_none());
}

#[test]
fn transmit_packet_numbers_stop_at_the_48_bit_limit_without_wrapping() {
    let step = CcmpPacketNumberStep::new(3).unwrap();
    let mut allocator = CcmpTxPacketNumber::continuing_after(
        step,
        CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX - 3).unwrap(),
    );
    assert_eq!(
        allocator.allocate(),
        Ok(CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX).unwrap())
    );
    assert_eq!(
        allocator.allocate(),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
    assert_eq!(
        allocator.last(),
        CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX).unwrap()
    );
    let mut near = CcmpTxPacketNumber::continuing_after(
        step,
        CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX - 2).unwrap(),
    );
    assert_eq!(near.allocate(), Err(CcmpTxPacketNumberError::Exhausted));
}
