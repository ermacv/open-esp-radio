use super::*;

macro_rules! rx_request {
    ($interface:expr, $peer:expr, $token:expr, $tid:expr, $immediate:expr, $window:expr, $timeout:expr, $start:expr) => {
        RxBlockAckRequest {
            interface: $interface,
            peer: $peer,
            dialog_token: $token,
            tid: $tid,
            immediate: $immediate,
            requested_window: $window,
            timeout_tu: $timeout,
            starting_sequence: $start,
        }
    };
}

/// Integration limits strictly below the hardware window of the active
/// build profile; the reduced-memory profile uses an eight-entry window.
const QUARTER_WINDOW: u16 = RX_BLOCK_ACK_MAX_WINDOW / 4;
const HALF_WINDOW: u16 = RX_BLOCK_ACK_MAX_WINDOW / 2;

fn frame(sequence: u16, slot: u8) -> RxAmpduMpdu {
    RxAmpduMpdu {
        sequence: SequenceNumber::new(sequence).unwrap(),
        slot,
    }
}

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
fn reset_clears_sessions_without_widening_the_integration_limit() {
    let peer = [2, 0, 0, 0, 0, 1];
    let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(QUARTER_WINDOW).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            7,
            0,
            true,
            64,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    sessions.commit(activation).unwrap();
    assert!(sessions.snapshots().iter().any(Option::is_some));

    sessions.reset_after_hardware_reset();

    assert_eq!(sessions.maximum_window(), QUARTER_WINDOW);
    assert!(sessions.snapshots().iter().all(Option::is_none));
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            8,
            0,
            true,
            64,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(activation.negotiated().window, QUARTER_WINDOW);
    assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
}

#[test]
fn in_order_frames_are_released_immediately() {
    let mut reorder =
        RxBlockAckReorder::new(SequenceNumber::new(10).unwrap(), RX_BLOCK_ACK_MAX_WINDOW).unwrap();
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
        RxBlockAckReorderState::<65>::new(SequenceNumber::new(100).unwrap(), QUARTER_WINDOW)
            .unwrap();
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
        RxBlockAckReorder::new(SequenceNumber::new(100).unwrap(), RX_BLOCK_ACK_MAX_WINDOW).unwrap();
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
#[cfg(not(feature = "rx-ba-window-8"))]
fn window_advance_releases_owned_frames_and_counts_missing_without_long_loop() {
    let mut reorder =
        RxBlockAckReorder::new(SequenceNumber::new(0).unwrap(), RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    reorder.ingest(frame(2, 2)).unwrap();
    reorder.ingest(frame(31, 31)).unwrap();
    let release = reorder.ingest(frame(1000, 1)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(2, 2), frame(31, 31)]
    );
    let expected_advance = 1000 - RX_BLOCK_ACK_MAX_WINDOW + 1;
    assert_eq!(release.missing, expected_advance - 2);
    assert!(release.buffered);
    assert_eq!(
        reorder.next_sequence(),
        SequenceNumber::new(expected_advance).unwrap()
    );
}

#[test]
fn async_expiry_skips_only_the_current_gap() {
    let mut reorder = RxBlockAckReorder::new(SequenceNumber::new(20).unwrap(), 8).unwrap();
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
    let mut reorder = RxBlockAckReorder::new(SequenceNumber::new(0x0fff).unwrap(), 8).unwrap();
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
    let mut reorder = RxBlockAckReorder::new(SequenceNumber::new(10).unwrap(), 8).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Ok(true)
    );
    reorder.ingest(frame(20, 0)).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Err(RxAmpduError::DuplicateSequence(
            SequenceNumber::new(20).unwrap()
        ))
    );

    let mut singleton =
        RxBlockAckReorderState::<1>::new(SequenceNumber::new(10).unwrap(), 1).unwrap();
    assert_eq!(
        singleton.retains_on_ingest(SequenceNumber::new(20).unwrap()),
        Ok(false)
    );
    assert!(!singleton.ingest(frame(20, 0)).unwrap().buffered);
}

#[test]
fn a_slot_index_cannot_be_owned_twice() {
    let mut reorder = RxBlockAckReorder::new(SequenceNumber::new(1).unwrap(), 8).unwrap();
    reorder.ingest(frame(2, 4)).unwrap();
    assert_eq!(
        reorder.ingest(frame(3, 4)),
        Err(RxAmpduError::SlotAlreadyOwned(4))
    );
}

#[test]
fn esf_slot_id_is_independent_of_reorder_window_index() {
    let mut reorder =
        RxBlockAckReorder::new(SequenceNumber::new(1).unwrap(), RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    let highest_valid = (RX_REORDER_SLOT_ID_CAPACITY - 1) as u8;
    let first_invalid = RX_REORDER_SLOT_ID_CAPACITY as u8;
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
        Err(RxAmpduError::InvalidSlot(first_invalid))
    );
}

#[test]
fn integration_can_bind_the_reorder_to_its_exact_slot_domain() {
    let mut reorder =
        RxBlockAckReorderState::<40>::new(SequenceNumber::new(1).unwrap(), 8).unwrap();
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
        Err(RxAmpduError::InvalidSlot(40))
    );
}

#[test]
fn window_cannot_exceed_the_owned_reorder_slot_pool() {
    assert!(
        RxBlockAckReorder::new(SequenceNumber::new(0).unwrap(), RX_BLOCK_ACK_MAX_WINDOW).is_ok()
    );
    assert!(matches!(
        RxBlockAckReorder::new(SequenceNumber::new(0).unwrap(), RX_BLOCK_ACK_MAX_WINDOW + 1),
        Err(RxAmpduError::InvalidWindow(_))
    ));
}

#[test]
fn stop_releases_every_owned_slot_in_sequence_order() {
    let mut reorder = RxBlockAckReorder::new(SequenceNumber::new(4094).unwrap(), 8).unwrap();
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
fn station_rx_sessions_bind_protocol_window_hardware_bank_and_response() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            17,
            7,
            true,
            1023,
            0,
            SequenceNumber::new(0x0abc).unwrap()
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(
        activation.negotiated(),
        RxBlockAckSnapshot {
            hardware_index: 0,
            interface: MacInterface::Station,
            peer,
            tid: 7,
            window: RX_BLOCK_ACK_MAX_WINDOW,
            starting_sequence: SequenceNumber::new(0x0abc).unwrap(),
        }
    );
    assert_eq!(
        activation.hardware(),
        S31RxBlockAckAgreement {
            hardware_index: 0,
            interface: MacInterface::Station,
            peer,
            tid: 7,
            starting_sequence: SequenceNumber::new(0x0abc).unwrap(),
            window: RX_BLOCK_ACK_MAX_WINDOW,
        }
    );
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(activation.response_body()),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 17,
            status: 0,
            tid: 7,
            immediate: true,
            amsdu: false,
            window: RX_BLOCK_ACK_MAX_WINDOW,
            timeout_tu: 0,
        })
    );
    assert!(matches!(
        sessions.begin_pending(),
        Err(RxBlockAckSessionsError::ActivationBusy)
    ));
    let snapshot = sessions.commit(activation).unwrap();
    assert_eq!(sessions.snapshots()[0], Some(snapshot));
    assert_eq!(
        sessions.stop(MacInterface::Station, peer, 7),
        Some(snapshot)
    );
    assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
}

#[test]
fn integration_can_narrow_the_negotiated_rx_window_without_changing_hardware_geometry() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(HALF_WINDOW).unwrap();
    assert_eq!(sessions.maximum_window(), HALF_WINDOW);
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            17,
            0,
            true,
            64,
            0,
            SequenceNumber::new(123).unwrap()
        ))
        .unwrap();

    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(activation.negotiated().window, HALF_WINDOW);
    assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(activation.response_body()),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 17,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: false,
            window: HALF_WINDOW,
            timeout_tu: 0,
        })
    );

    assert!(matches!(
        RxBlockAckSessions::<1>::with_maximum_window(0),
        Err(RxBlockAckSessionsError::InvalidWindow(0))
    ));
    assert!(matches!(
        RxBlockAckSessions::<1>::with_maximum_window(RX_BLOCK_ACK_MAX_WINDOW + 1),
        Err(RxBlockAckSessionsError::InvalidWindow(window))
            if window == RX_BLOCK_ACK_MAX_WINDOW + 1
    ));
}

#[test]
fn replacement_and_cancel_remove_the_previous_hardware_owner() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            32,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    let first = sessions.begin_pending().unwrap().unwrap();
    let first_snapshot = sessions.commit(first).unwrap();

    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            2,
            0,
            true,
            16,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();
    let replacement = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(replacement.replaced(), Some(first_snapshot));
    assert_eq!(replacement.hardware().hardware_index, 0);
    sessions.cancel(replacement).unwrap();
    assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
}

#[test]
fn station_rx_sessions_reject_every_unsupported_request_class() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            false,
            32,
            0,
            SequenceNumber::new(0).unwrap()
        )),
        Err(RxBlockAckSessionsError::DelayedPolicyUnsupported)
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            8,
            true,
            32,
            0,
            SequenceNumber::new(0).unwrap()
        )),
        Err(RxBlockAckSessionsError::InvalidTid(8))
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            0,
            0,
            SequenceNumber::new(0).unwrap()
        )),
        Err(RxBlockAckSessionsError::InvalidWindow(0))
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            32,
            1,
            SequenceNumber::new(0).unwrap()
        )),
        Err(RxBlockAckSessionsError::NonzeroTimeout(1))
    );
}

#[test]
fn access_point_peers_with_the_same_tid_receive_distinct_hardware_banks() {
    let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();

    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            0,
            true,
            32,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    let first = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(first.hardware().interface, MacInterface::AccessPoint);
    assert_eq!(first.hardware().hardware_index, 0);
    sessions.commit(first).unwrap();

    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            2,
            0,
            true,
            32,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();
    let second = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(second.hardware().hardware_index, 1);
    sessions.commit(second).unwrap();

    assert!(
        sessions
            .stop(MacInterface::AccessPoint, first_peer, 0)
            .is_some()
    );
    assert!(
        sessions
            .stop(MacInterface::AccessPoint, second_peer, 0)
            .is_some()
    );
}

#[test]
fn station_and_access_point_with_the_same_peer_tid_use_distinct_banks() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<2>::new();

    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            16,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            2,
            0,
            true,
            16,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();

    let station = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(station.negotiated().interface, MacInterface::Station);
    assert_eq!(station.negotiated().hardware_index, 0);
    sessions.commit(station).unwrap();

    let access_point = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(
        access_point.negotiated().interface,
        MacInterface::AccessPoint
    );
    assert_eq!(access_point.negotiated().hardware_index, 1);
    sessions.commit(access_point).unwrap();

    assert_eq!(
        sessions.snapshots()[0].unwrap().interface,
        MacInterface::Station
    );
    assert_eq!(
        sessions.snapshots()[1].unwrap().interface,
        MacInterface::AccessPoint
    );
}

#[test]
fn preparing_access_point_preserves_station_banks() {
    let station_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let access_point_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<2>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            station_peer,
            1,
            0,
            true,
            16,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    let station = sessions.begin_pending().unwrap().unwrap();
    let station = sessions.commit(station).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            access_point_peer,
            2,
            0,
            true,
            16,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();

    sessions
        .prepare_interface(MacInterface::AccessPoint)
        .unwrap();

    assert_eq!(
        sessions.snapshots_for(MacInterface::Station)[0],
        Some(station)
    );
    assert!(
        sessions
            .snapshots_for(MacInterface::AccessPoint)
            .into_iter()
            .all(|entry| entry.is_none())
    );
    assert!(matches!(sessions.begin_pending(), Ok(None)));
    assert_eq!(
        sessions.prepare_interface(MacInterface::Station),
        Err(RxBlockAckSessionsError::InterfaceActive(
            MacInterface::Station
        ))
    );
}

#[test]
fn request_remains_pending_while_every_hardware_bank_is_owned() {
    let mut sessions = RxBlockAckSessions::<{ RX_BLOCK_ACK_BANK_COUNT + 1 }>::new();
    for index in 0..RX_BLOCK_ACK_BANK_COUNT {
        let peer = [2, 0, 0, 0, 0, index as u8];
        sessions
            .offer(rx_request!(
                MacInterface::AccessPoint,
                peer,
                index as u8,
                0,
                true,
                32,
                0,
                SequenceNumber::new(index as u16).unwrap()
            ))
            .unwrap();
        let activation = sessions.begin_pending().unwrap().unwrap();
        sessions.commit(activation).unwrap();
    }

    let waiting_peer = [2, 0, 0, 0, 1, 0];
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            waiting_peer,
            9,
            0,
            true,
            32,
            0,
            SequenceNumber::new(100).unwrap()
        ))
        .unwrap();
    assert!(matches!(
        sessions.begin_pending(),
        Err(RxBlockAckSessionsError::NoFreeHardwareBank)
    ));

    let released_peer = [2, 0, 0, 0, 0, 0];
    assert!(
        sessions
            .stop(MacInterface::AccessPoint, released_peer, 0)
            .is_some()
    );
    let activation = sessions
        .begin_pending()
        .unwrap()
        .expect("bank release must admit the retained pending request");
    assert_eq!(activation.negotiated().peer, waiting_peer);
}

#[test]
fn explicit_decline_removes_only_the_selected_pending_request() {
    let first_peer = [2, 0, 0, 0, 0, 1];
    let second_peer = [2, 0, 0, 0, 0, 2];
    let mut sessions = RxBlockAckSessions::<2>::new();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            1,
            true,
            16,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            2,
            1,
            true,
            16,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();

    assert!(sessions.discard_pending(MacInterface::AccessPoint, first_peer, 1));
    assert!(!sessions.discard_pending(MacInterface::AccessPoint, first_peer, 1));
    let activation = sessions
        .begin_pending()
        .unwrap()
        .expect("other peer request remains pending");
    assert_eq!(activation.negotiated().peer, second_peer);
}

#[test]
fn peer_teardown_removes_pending_and_active_agreements_only_for_that_peer() {
    let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            0,
            true,
            32,
            0,
            SequenceNumber::new(10).unwrap()
        ))
        .unwrap();
    let active = sessions.begin_pending().unwrap().unwrap();
    sessions.commit(active).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            2,
            1,
            true,
            32,
            0,
            SequenceNumber::new(20).unwrap()
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            3,
            1,
            true,
            32,
            0,
            SequenceNumber::new(30).unwrap()
        ))
        .unwrap();

    let stopped = sessions.stop_peer(MacInterface::AccessPoint, first_peer);
    assert_eq!(stopped.into_iter().flatten().count(), 1);
    let remaining = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(remaining.negotiated().peer, second_peer);
}

#[test]
fn a_block_ack_request_releases_frames_before_its_start_then_the_run_from_it() {
    let mut state = RxBlockAckReorderState::<40>::new(SequenceNumber::new(10).unwrap(), 8).unwrap();
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
    let mut state =
        RxBlockAckReorderState::<40>::new(SequenceNumber::new(100).unwrap(), 8).unwrap();
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
