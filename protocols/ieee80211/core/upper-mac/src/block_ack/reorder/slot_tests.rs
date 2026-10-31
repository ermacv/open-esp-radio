//! Reorder behavior across sequence windows and bounded backing-slot domains.

use super::*;

fn frame(sequence: u16, slot: u8) -> RxReorderMpdu {
    RxReorderMpdu {
        sequence: SequenceNumber::new(sequence).unwrap(),
        slot,
    }
}

#[test]
fn in_order_frames_are_released_immediately() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(10).unwrap(), 64).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(10).unwrap()),
        Ok(false)
    );
    let release = reorder.ingest(frame(10, 0)).unwrap();
    assert_eq!(release.iter().collect::<std::vec::Vec<_>>(), [frame(10, 0)]);
    assert_eq!(reorder.next_sequence(), SequenceNumber::new(11).unwrap());
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn immediate_ingest_avoids_a_release_list_only_without_a_buffered_successor() {
    let mut reorder =
        RxReorderBuffer::<64, 65>::new(SequenceNumber::new(100).unwrap(), 16).unwrap();
    assert_eq!(
        reorder.try_ingest_immediate(frame(100, 64)),
        Ok(Some(frame(100, 64)))
    );
    assert_eq!(reorder.next_sequence(), SequenceNumber::new(101).unwrap());
    assert_eq!(reorder.occupied(), 0);

    assert_eq!(reorder.try_ingest_immediate(frame(103, 64)), Ok(None));
    assert_eq!(reorder.next_sequence(), SequenceNumber::new(101).unwrap());

    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert_eq!(reorder.try_ingest_immediate(frame(101, 64)), Ok(None));
    let release = reorder.ingest(frame(101, 64)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(101, 64), frame(102, 2)]
    );
}

#[test]
fn gap_is_buffered_and_then_released_in_sequence_order() {
    let mut reorder =
        RxReorderBuffer::<64, 34>::new(SequenceNumber::new(100).unwrap(), 64).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(102).unwrap()),
        Ok(true)
    );
    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(101).unwrap()),
        Ok(true)
    );
    assert!(reorder.ingest(frame(101, 1)).unwrap().buffered);
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(100).unwrap()),
        Ok(false)
    );
    let release = reorder.ingest(frame(100, 0)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(100, 0), frame(101, 1), frame(102, 2)]
    );
    assert_eq!(reorder.next_sequence(), SequenceNumber::new(103).unwrap());
}

#[test]
fn window_advance_releases_owned_frames_and_counts_missing_without_long_loop() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(0).unwrap(), 64).unwrap();
    reorder.ingest(frame(2, 2)).unwrap();
    reorder.ingest(frame(31, 31)).unwrap();
    let release = reorder.ingest(frame(1000, 1)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(2, 2), frame(31, 31)]
    );
    let expected_advance = 1000 - 64 + 1;
    assert_eq!(release.missing, expected_advance - 2);
    assert!(release.buffered);
    assert_eq!(
        reorder.next_sequence(),
        SequenceNumber::new(expected_advance).unwrap()
    );
}

#[test]
fn async_expiry_skips_only_the_current_gap() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(20).unwrap(), 8).unwrap();
    reorder.ingest(frame(22, 2)).unwrap();
    reorder.ingest(frame(23, 3)).unwrap();
    let release = reorder.expire_gap();
    assert_eq!(release.missing, 2);
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(22, 2), frame(23, 3)]
    );
    assert_eq!(reorder.next_sequence(), SequenceNumber::new(24).unwrap());
}

#[test]
fn sequence_wrap_and_stale_rejection_are_unambiguous() {
    let mut reorder =
        RxReorderBuffer::<64, 34>::new(SequenceNumber::new(0x0fff).unwrap(), 8).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(0).unwrap()),
        Ok(true)
    );
    reorder.ingest(frame(0, 1)).unwrap();
    let release = reorder.ingest(frame(0x0fff, 0)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(0x0fff, 0), frame(0, 1)]
    );
    let stale = reorder.ingest(frame(0x0fff, 2)).unwrap();
    assert_eq!(stale.rejected, Some(frame(0x0fff, 2)));
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(0x0fff).unwrap()),
        Ok(false)
    );
}

#[test]
fn retention_prediction_matches_window_advance_and_duplicate_edges() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(10).unwrap(), 8).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Ok(true)
    );
    reorder.ingest(frame(20, 0)).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Err(RxReorderError::DuplicateSequence(
            SequenceNumber::new(20).unwrap()
        ))
    );

    let mut singleton = RxReorderBuffer::<64, 1>::new(SequenceNumber::new(10).unwrap(), 1).unwrap();
    assert_eq!(
        singleton.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Ok(false)
    );
    assert!(!singleton.ingest(frame(20, 0)).unwrap().buffered);
}

#[test]
fn a_slot_index_cannot_be_owned_twice() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(1).unwrap(), 8).unwrap();
    reorder.ingest(frame(2, 4)).unwrap();
    assert_eq!(
        reorder.ingest(frame(3, 4)),
        Err(RxReorderError::SlotAlreadyOwned(4))
    );
}

#[test]
fn backing_slot_id_is_independent_of_reorder_window_index() {
    let mut reorder = RxReorderBuffer::<64, 34>::new(SequenceNumber::new(1).unwrap(), 64).unwrap();
    let highest_valid = (34 - 1) as u8;
    let first_invalid = 34 as u8;
    assert_eq!(
        reorder
            .ingest(frame(1, highest_valid))
            .unwrap()
            .iter()
            .collect::<std::vec::Vec<_>>(),
        [frame(1, highest_valid)]
    );
    assert_eq!(
        reorder.ingest(frame(2, first_invalid)),
        Err(RxReorderError::InvalidSlot(first_invalid))
    );
}

#[test]
fn integration_can_bind_the_reorder_to_its_exact_slot_domain() {
    let mut reorder = RxReorderBuffer::<64, 40>::new(SequenceNumber::new(1).unwrap(), 8).unwrap();
    assert_eq!(
        reorder
            .ingest(frame(1, 39))
            .unwrap()
            .iter()
            .collect::<std::vec::Vec<_>>(),
        [frame(1, 39)]
    );
    assert_eq!(
        reorder.ingest(frame(2, 40)),
        Err(RxReorderError::InvalidSlot(40))
    );
}

#[test]
fn window_cannot_exceed_the_owned_reorder_slot_pool() {
    assert!(RxReorderBuffer::<64, 34>::new(SequenceNumber::new(0).unwrap(), 64).is_ok());
    assert!(matches!(
        RxReorderBuffer::<64, 34>::new(SequenceNumber::new(0).unwrap(), 64 + 1),
        Err(RxReorderError::InvalidWindow(_))
    ));
}

#[test]
fn stop_releases_every_owned_slot_in_sequence_order() {
    let mut reorder =
        RxReorderBuffer::<64, 34>::new(SequenceNumber::new(4094).unwrap(), 8).unwrap();
    reorder.ingest(frame(1, 3)).unwrap();
    reorder.ingest(frame(4095, 1)).unwrap();
    let release = reorder.stop();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(4095, 1), frame(1, 3)]
    );
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn a_block_ack_request_releases_frames_before_its_start_then_the_run_from_it() {
    let mut state = RxReorderBuffer::<64, 40>::new(SequenceNumber::new(10).unwrap(), 8).unwrap();
    // 10 and 11 are missing; 12, 14 and 15 are buffered.
    for (sequence, slot) in [(12, 2), (14, 4), (15, 5)] {
        assert!(state.ingest(frame(sequence, slot)).unwrap().buffered);
    }
    let release = state
        .move_window_to(SequenceNumber::new(13).unwrap())
        .expect("a forward starting sequence moves the window");
    // 12 lies before the new start; 13 is still missing, so 14 and 15 stay.
    assert_eq!(release.iter().collect::<std::vec::Vec<_>>(), [frame(12, 2)]);
    assert_eq!(release.missing, 2);
    assert_eq!(state.next_sequence(), SequenceNumber::new(13).unwrap());

    let release = state
        .move_window_to(SequenceNumber::new(14).unwrap())
        .unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(14, 4), frame(15, 5)]
    );
    assert_eq!(state.next_sequence(), SequenceNumber::new(16).unwrap());
    assert_eq!(state.occupied(), 0);
}

#[test]
fn a_block_ack_request_at_or_behind_the_window_start_is_ignored() {
    let mut state = RxReorderBuffer::<64, 40>::new(SequenceNumber::new(100).unwrap(), 8).unwrap();
    assert!(state.ingest(frame(102, 2)).unwrap().buffered);
    assert!(
        state
            .move_window_to(SequenceNumber::new(100).unwrap())
            .is_none()
    );
    assert!(
        state
            .move_window_to(SequenceNumber::new(99).unwrap())
            .is_none()
    );
    // Half the sequence space behind wraps to "behind", not "ahead".
    assert!(
        state
            .move_window_to(SequenceNumber::new(100 + 2048).unwrap())
            .is_none()
    );
    assert_eq!(state.next_sequence(), SequenceNumber::new(100).unwrap());
    assert_eq!(state.occupied(), 1);
}
