use super::*;

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

/// The input's first eight octets as a CCMP header and receive sequence;
/// each following pair as a lane and a packet number's low octet.
fn parse_and_check(bytes: &[u8]) {
    let Some((header, rest)) = bytes.split_first_chunk::<CCMP_HEADER_LEN>() else {
        return;
    };
    if let Ok(parsed) = CcmpHeader::parse(*header) {
        assert_eq!(parsed.encode(), *header);
        let _ = (parsed.packet_number().value(), parsed.key_id().value());
    }
    let _ = CcmpPacketNumber::from_header_bytes(*header);
    let Ok(mut state) = CcmpRxReplayState::from_receive_sequence(*header) else {
        return;
    };
    for &[lane, low] in rest.as_chunks::<2>().0 {
        let lane = if lane == 0xff {
            CcmpReplayLane::NonQos
        } else {
            CcmpReplayLane::Tid(lane)
        };
        let packet_number = CcmpPacketNumber::from_header_bytes([low, 0, 0, 0, 0, 0, 0, 0]);
        let _ = state.highest(lane);
        match state.prepare(lane, packet_number) {
            Ok(candidate) if low & 1 == 0 => candidate.commit(),
            Ok(_) | Err(_) => {}
        }
        let _ = state.commit_immediate(lane, packet_number);
    }
}

fn run(count: usize) {
    let header = CcmpHeader::new(
        CcmpPacketNumber::new(0x6655_4433_2211).unwrap(),
        CcmpKeyId::new(2).unwrap(),
    )
    .encode();
    let sequence = [1, 0, 0, 0, 0, 0, 0, 0, 0, 2, 7, 3, 0xff, 4, 16, 5, 15, 6];
    oer_ieee80211_wire::fuzz::inputs(&[&header, &sequence], count, 64, parse_and_check);
}

#[test]
fn header_parse_and_replay_lanes_never_panic() {
    run(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn header_parse_and_replay_lanes_never_panic_full() {
    run(oer_ieee80211_wire::fuzz::FULL);
}
