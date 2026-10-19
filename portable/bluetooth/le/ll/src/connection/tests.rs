use super::*;

const INITIATOR: [u8; 6] = [1, 2, 3, 4, 5, 6];
const ADVERTISER: [u8; 6] = [7, 8, 9, 10, 11, 12];

pub(super) fn connection_request(
    channel_selection_two: bool,
) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = CONNECT_IND_TYPE
        | if channel_selection_two {
            CHANNEL_SELECTION_TWO
        } else {
            0
        };
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&INITIATOR);
    pdu[8..14].copy_from_slice(&ADVERTISER);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
    pdu[35] = 5 | (4 << 5);
    pdu
}

fn complete_missed(connection: LePeripheralConnection) -> LePeripheralConnectionEventCompleted {
    connection
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed)
}

#[test]
fn establishment_expires_after_six_misses_but_a_packet_in_event_six_establishes() {
    for last_activity in [
        LePeripheralConnectionEventPeerActivity::Missed,
        LePeripheralConnectionEventPeerActivity::Observed,
    ] {
        let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
        let mut connection = LePeripheralConnection::from_request(
            request,
            LeChannelSelectionAlgorithm::AlgorithmTwo,
        );
        for counter in 0..6 {
            let activity = if counter == 5 {
                last_activity
            } else {
                LePeripheralConnectionEventPeerActivity::Missed
            };
            let completed = connection
                .prepare_event()
                .into_submitted()
                .complete(activity);
            assert_eq!(completed.event_counter(), counter);
            assert_eq!(
                completed.establishment_failed(),
                counter == 5 && last_activity == LePeripheralConnectionEventPeerActivity::Missed
            );
            connection = completed.into_connection();
        }
    }
}

#[test]
fn complete_connect_ind_becomes_semantic_peripheral_input() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();

    assert_eq!(request.initiator().wire_bytes(), INITIATOR);
    assert_eq!(request.advertiser().wire_bytes(), ADVERTISER);
    assert_eq!(request.access_address().value(), 0xa1b2_c3d4);
    assert_eq!(request.crc_initialization().value(), 0x11_2233);
    assert_eq!(request.timing().interval_micros(), 30_000);
    assert_eq!(request.timing().first_window_start_micros(), 2_500);
    assert_eq!(request.timing().first_window_end_micros(), 5_000);
    assert_eq!(request.channel_map().used_channel_count(), 37);
    assert_eq!(request.hop_increment(), 5);
    assert_eq!(request.sleep_clock_accuracy().encoded(), 4);
    assert_eq!(request.sleep_clock_accuracy().worst_case_ppm(), 75);
    assert_eq!(
        request.channel_selection(),
        LeChannelSelectionAlgorithm::AlgorithmTwo
    );
}

#[test]
fn every_sleep_clock_accuracy_class_exposes_its_worst_case_ppm() {
    for (encoded, expected_ppm) in [500, 250, 150, 100, 75, 50, 30, 20].into_iter().enumerate() {
        let mut pdu = connection_request(false);
        pdu[35] = 5 | ((encoded as u8) << 5);
        let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();

        assert_eq!(
            request.sleep_clock_accuracy().worst_case_ppm(),
            expected_ppm
        );
    }
}

#[test]
fn malformed_request_fails_before_connection_ownership_exists() {
    let mut pdu = connection_request(false);
    pdu[0] |= HEADER_RESERVED;
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet)
    );

    let mut pdu = connection_request(false);
    pdu[21] = 0;
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::Timing(
            LeConnectionTimingError::WindowSizeOutsideRange
        ))
    );

    let mut pdu = connection_request(false);
    pdu[30..35].copy_from_slice(&[1, 0, 0, 0, 0]);
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::ChannelMap(
            LeDataChannelMapError::FewerThanTwoUsedChannels
        ))
    );
}

#[test]
fn access_address_rejects_intrinsically_ambiguous_patterns() {
    assert_eq!(
        LeUncodedAccessAddress::new(ADVERTISING_ACCESS_ADDRESS),
        Err(LeUncodedAccessAddressError::AdvertisingAddressOrOneBitAway)
    );
    assert_eq!(
        LeUncodedAccessAddress::new(0x1212_1212),
        Err(LeUncodedAccessAddressError::AllOctetsEqual)
    );
    assert!(LeUncodedAccessAddress::new(0xa1b2_c3d4).is_ok());
}

#[test]
fn supervision_relation_is_strict_and_lossless() {
    assert_eq!(
        LeConnectionTiming::new(1, 0, 24, 4, 30),
        Err(LeConnectionTimingError::SupervisionTimeoutTooShort)
    );
    assert!(LeConnectionTiming::new(1, 0, 24, 4, 31).is_ok());
}

#[test]
fn csa2_matches_bluetooth_sig_event_samples() {
    let selector = LeChannelSelectionAlgorithmTwo {
        channel_identifier: 0x305f,
        channel_map: LeDataChannelMap::all(),
    };
    assert_eq!(selector.select(0).get(), 25);
    assert_eq!(selector.select(1).get(), 20);
    assert_eq!(selector.select(2).get(), 6);
    assert_eq!(selector.select(3).get(), 21);

    let sparse = LeChannelSelectionAlgorithmTwo {
        channel_identifier: 0x305f,
        channel_map: LeDataChannelMap::new([0x00, 0x06, 0xe0, 0x00, 0x1e]).unwrap(),
    };
    assert_eq!(sparse.select(6).get(), 23);
    assert_eq!(sparse.select(7).get(), 9);
    assert_eq!(sparse.select(8).get(), 34);
}

#[test]
fn csa1_commits_hop_only_after_exact_event_completion() {
    let mut pdu = connection_request(false);
    pdu[30..35].copy_from_slice(&[0x06, 0, 0, 0, 0]);
    let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();
    let connection = LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );

    let first = connection.prepare_event();
    assert_eq!(first.event_counter(), 0);
    assert_eq!(first.channel().get(), 2);
    assert_eq!(first.first_transmit_window_micros(), Some((2_500, 5_000)));

    let retry = first.cancel().prepare_event();
    assert_eq!(retry.event_counter(), 0);
    assert_eq!(retry.channel().get(), 2);

    let completed = retry
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(completed.event_counter(), 0);
    assert_eq!(completed.channel().get(), 2);
    assert_eq!(
        completed.peer_activity(),
        LePeripheralConnectionEventPeerActivity::Missed
    );
    assert_eq!(
        completed.connection_state(),
        LePeripheralConnectionState::Created
    );

    let second = completed.into_connection().prepare_event();
    assert_eq!(second.event_counter(), 1);
    assert_eq!(second.channel().get(), 1);
    assert_eq!(second.first_transmit_window_micros(), None);
}

#[test]
fn observed_and_missed_events_advance_once_and_retain_lifecycle_anchors() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let connection = LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );
    assert_eq!(connection.state(), LePeripheralConnectionState::Created);
    assert_eq!(connection.next_event_distance_from_establishment(), None);
    assert_eq!(
        connection.next_event_distance_from_supervision_anchor(),
        None
    );

    let first = connection.prepare_event();
    let first_channel = first.channel();
    let completed = first
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(completed.event_counter(), 0);
    assert_eq!(completed.channel(), first_channel);
    assert_eq!(
        completed.connection_state(),
        LePeripheralConnectionState::Established {
            establishment_event_counter: 0,
            supervision_anchor_event_counter: 0,
        }
    );
    let connection = completed.into_connection();
    assert_eq!(connection.event_counter(), 1);
    assert_eq!(connection.next_event_distance_from_establishment(), Some(1));
    assert_eq!(
        connection.next_event_distance_from_supervision_anchor(),
        Some(1)
    );

    let second = connection.prepare_event();
    let second_channel = second.channel();
    let completed = second
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(completed.event_counter(), 1);
    assert_eq!(completed.channel(), second_channel);
    assert_eq!(
        completed.connection_state(),
        LePeripheralConnectionState::Established {
            establishment_event_counter: 0,
            supervision_anchor_event_counter: 0,
        }
    );
    let connection = completed.into_connection();
    assert_eq!(connection.event_counter(), 2);
    assert_eq!(connection.next_event_distance_from_establishment(), Some(2));
    assert_eq!(
        connection.next_event_distance_from_supervision_anchor(),
        Some(2)
    );

    let third = connection.prepare_event();
    let completed = third
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(completed.event_counter(), 2);
    assert_eq!(
        completed.connection_state(),
        LePeripheralConnectionState::Established {
            establishment_event_counter: 0,
            supervision_anchor_event_counter: 2,
        }
    );
    let connection = completed.into_connection();
    assert_eq!(connection.event_counter(), 3);
    assert_eq!(connection.next_event_distance_from_establishment(), Some(3));
    assert_eq!(
        connection.next_event_distance_from_supervision_anchor(),
        Some(1)
    );
}

#[test]
fn event_counter_wraps_without_reusing_an_in_flight_owner() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let mut connection = LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );
    connection.event_counter = u16::MAX;

    let in_flight = connection.prepare_event().into_submitted();
    assert_eq!(in_flight.event_counter(), u16::MAX);
    let completed = in_flight.complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(completed.event_counter(), u16::MAX);
    assert_eq!(completed.into_connection().event_counter(), 0);
}

#[test]
fn recurring_event_delta_is_nonzero_and_rejects_skipped_overflow() {
    assert_eq!(LePeripheralConnectionEventDelta::new(0), None);
    assert_eq!(
        LePeripheralConnectionEventDelta::from_skipped(0)
            .unwrap()
            .get(),
        1
    );
    assert_eq!(
        LePeripheralConnectionEventDelta::from_skipped(6)
            .unwrap()
            .skipped(),
        6
    );
    assert_eq!(
        LePeripheralConnectionEventDelta::from_skipped(u16::MAX),
        None
    );
}

#[test]
fn delta_one_prepares_immediate_csa1_successor_without_double_advance() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(false)).unwrap();
    let completed = complete_missed(LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let delta = LePeripheralConnectionEventDelta::from_skipped(0).unwrap();

    let provisional = completed.prepare_recurring_event(delta);
    assert_eq!(provisional.delta(), delta);
    assert_eq!(provisional.request(), request);
    assert_eq!(provisional.event_counter(), 1);
    assert_eq!(provisional.channel().get(), 10);
    assert_eq!(provisional.timing(), request.timing());

    let prepared = provisional.commit();
    assert_eq!(prepared.event_counter(), 1);
    assert_eq!(prepared.channel().get(), 10);
    let completed = prepared
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(completed.event_counter(), 1);
    assert_eq!(completed.into_connection().event_counter(), 2);
}

#[test]
fn csa1_skipped_events_match_repeated_advancement_and_commit_once() {
    let mut pdu = connection_request(false);
    pdu[30..35].copy_from_slice(&[0x06, 0, 0, 0, 0]);
    let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();

    let completed = complete_missed(LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let delta = LePeripheralConnectionEventDelta::from_skipped(3).unwrap();
    let provisional = completed.prepare_recurring_event(delta);

    let mut reference = complete_missed(LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    ))
    .into_connection();
    for _ in 0..3 {
        reference = complete_missed(reference).into_connection();
    }
    let reference_target = reference.prepare_event();
    assert_eq!(
        provisional.event_counter(),
        reference_target.event_counter()
    );
    assert_eq!(provisional.channel(), reference_target.channel());
    assert_eq!(provisional.event_counter(), 4);
    assert_eq!(provisional.channel().get(), 2);

    let prepared = provisional.commit();
    assert_eq!(prepared.event_counter(), reference_target.event_counter());
    assert_eq!(prepared.channel(), reference_target.channel());
    let completed = prepared
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    let next = completed.into_connection().prepare_event();
    assert_eq!(next.event_counter(), 5);
    assert_eq!(next.channel().get(), 1);
}

#[test]
fn recurring_candidate_cancel_restores_exact_completed_owner() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(false)).unwrap();
    let completed = complete_missed(LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let expected = complete_missed(LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let delta = LePeripheralConnectionEventDelta::from_skipped(3).unwrap();

    let restored = completed.prepare_recurring_event(delta).cancel();

    assert_eq!(restored, expected);
    let immediate = restored.into_connection().prepare_event();
    assert_eq!(immediate.event_counter(), 1);
    assert_eq!(immediate.channel().get(), 10);
}

#[test]
fn csa2_recurring_preview_selects_the_final_wrapped_counter() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let mut connection = LePeripheralConnection::from_request(
        request,
        crate::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );
    connection.event_counter = u16::MAX;
    let completed = complete_missed(connection);
    let delta = LePeripheralConnectionEventDelta::from_skipped(1).unwrap();

    let provisional = completed.prepare_recurring_event(delta);

    assert_eq!(provisional.event_counter(), 1);
    assert_eq!(
        provisional.channel(),
        LeChannelSelectionAlgorithmTwo::new(request.access_address(), request.channel_map())
            .select(1)
    );
    let prepared = provisional.commit();
    assert_eq!(prepared.event_counter(), 1);
    assert_eq!(
        prepared.channel(),
        LeChannelSelectionAlgorithmTwo::new(request.access_address(), request.channel_map())
            .select(1)
    );
}

#[test]
fn channel_map_update_applies_at_the_exact_csa1_instant() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(false)).unwrap();
    let mut completed = complete_missed(LePeripheralConnection::from_request(
        request,
        LeChannelSelectionAlgorithm::AlgorithmOne,
    ));
    let new_map = LeDataChannelMap::new([0x03, 0, 0, 0, 0]).unwrap();
    completed.schedule_channel_map_update(new_map, 6).unwrap();
    let mut connection = completed.into_connection();

    for event_counter in 1..6 {
        let prepared = connection.prepare_event();
        assert_eq!(prepared.event_counter(), event_counter);
        assert_eq!(prepared.request().channel_map(), request.channel_map());
        connection = prepared
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Missed)
            .into_connection();
    }

    let instant = connection.prepare_event();
    assert_eq!(instant.event_counter(), 6);
    assert_eq!(instant.request().channel_map(), new_map);
    assert_eq!(instant.channel().get(), 1);
    assert!(
        instant
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Missed)
            .channel_map_updated()
    );
}

#[test]
fn skipped_channel_map_instant_is_previewed_without_consuming_cancel_owner() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(false)).unwrap();
    let mut completed = complete_missed(LePeripheralConnection::from_request(
        request,
        LeChannelSelectionAlgorithm::AlgorithmOne,
    ));
    let new_map = LeDataChannelMap::new([0x03, 0, 0, 0, 0]).unwrap();
    completed.schedule_channel_map_update(new_map, 6).unwrap();
    let delta = LePeripheralConnectionEventDelta::new(6).unwrap();

    let restored = completed.prepare_recurring_event(delta).cancel();
    assert_eq!(
        restored
            .into_connection()
            .prepare_event()
            .request()
            .channel_map(),
        request.channel_map()
    );

    let mut completed = complete_missed(LePeripheralConnection::from_request(
        request,
        LeChannelSelectionAlgorithm::AlgorithmOne,
    ));
    completed.schedule_channel_map_update(new_map, 6).unwrap();
    let provisional = completed.prepare_recurring_event(delta);
    assert_eq!(provisional.event_counter(), 6);
    assert_eq!(provisional.channel().get(), 1);
    let prepared = provisional.commit();
    assert_eq!(prepared.request().channel_map(), new_map);
    assert_eq!(prepared.channel().get(), 1);
}

#[test]
fn channel_map_instant_wraps_and_rejects_past_or_colliding_updates() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let mut connection =
        LePeripheralConnection::from_request(request, LeChannelSelectionAlgorithm::AlgorithmTwo);
    connection.event_counter = 0xfffa;
    let mut completed = complete_missed(connection);
    let new_map = LeDataChannelMap::new([0x03, 0, 0, 0, 0]).unwrap();

    completed
        .schedule_channel_map_update(new_map, 0xfffa)
        .unwrap();
    assert_eq!(
        completed.connection.prepare_event().request().channel_map(),
        new_map
    );
    let mut connection =
        LePeripheralConnection::from_request(request, LeChannelSelectionAlgorithm::AlgorithmTwo);
    connection.event_counter = 0xfffa;
    let mut completed = complete_missed(connection);
    assert_eq!(
        completed.schedule_channel_map_update(new_map, 0x7ff9),
        Err(LePeripheralChannelMapUpdateError::InstantPassed)
    );
    completed.schedule_channel_map_update(new_map, 0).unwrap();
    assert_eq!(
        completed.schedule_channel_map_update(LeDataChannelMap::all(), 1),
        Err(LePeripheralChannelMapUpdateError::ProcedureAlreadyPending)
    );

    let prepared = completed
        .prepare_recurring_event(LePeripheralConnectionEventDelta::new(6).unwrap())
        .commit();
    assert_eq!(prepared.event_counter(), 0);
    assert_eq!(prepared.request().channel_map(), new_map);
    assert_eq!(
        prepared.channel(),
        LeChannelSelectionAlgorithmTwo::new(request.access_address(), new_map).select(0)
    );
}

#[test]
fn connection_update_commits_timing_only_at_the_exact_instant() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let mut completed = complete_missed(LePeripheralConnection::from_request(
        request,
        LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let updated = LeConnectionTiming::new(2, 1, 40, 3, 200).unwrap();
    completed.schedule_connection_update(updated, 4).unwrap();
    let mut connection = completed.into_connection();

    for event_counter in 1..3 {
        let prepared = connection.prepare_event();
        assert_eq!(prepared.event_counter(), event_counter);
        assert_eq!(prepared.timing(), request.timing());
        connection = prepared
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Missed)
            .into_connection();
    }

    // The event before the instant previews it, in flight and closed.
    let in_flight = connection.prepare_event().into_submitted();
    let following = super::LeFollowingEvent::Instant {
        previous: request.timing(),
        updated,
    };
    assert_eq!(in_flight.following_event(), following);
    let completed = in_flight.complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(completed.following_event(), following);
    let provisional =
        completed.prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());
    let transition = provisional.connection_timing_transition().unwrap();
    assert_eq!(transition.previous(), request.timing());
    assert_eq!(transition.updated(), updated);
    assert_eq!(transition.instant(), 4);
    assert!(transition.host_parameters_changed());
    assert_eq!(provisional.timing(), updated);
    assert_eq!(
        provisional.connection_timing_intervals(),
        Some(super::LeConnectionTimingIntervals {
            before: 1,
            after: 0
        })
    );
    // A longer step crosses the instant at its first event: one interval of
    // the previous timing, then two of the updated one.
    let restored = provisional.cancel();
    let longer =
        restored.prepare_recurring_event(LePeripheralConnectionEventDelta::new(3).unwrap());
    assert_eq!(longer.connection_timing_transition(), Some(transition));
    assert_eq!(
        longer.connection_timing_intervals(),
        Some(super::LeConnectionTimingIntervals {
            before: 1,
            after: 2
        })
    );
    let provisional = longer
        .cancel()
        .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());

    let restored = provisional.cancel();
    assert_eq!(restored.timing(), request.timing());
    let prepared = restored
        .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap())
        .commit();
    assert_eq!(prepared.event_counter(), 4);
    assert_eq!(prepared.timing(), updated);
    let applied = prepared
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(applied.connection_timing_transition(), Some(transition));
    assert_eq!(applied.timing(), updated);
}

#[test]
fn connection_anchor_move_is_not_a_host_parameter_change() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let mut completed = complete_missed(LePeripheralConnection::from_request(
        request,
        LeChannelSelectionAlgorithm::AlgorithmTwo,
    ));
    let timing = request.timing();
    let anchor_move = LeConnectionTiming::new(
        timing.window_size_units(),
        timing.window_offset_units() + 1,
        timing.interval_units(),
        timing.peripheral_latency(),
        timing.supervision_timeout_units(),
    )
    .unwrap();
    completed
        .schedule_connection_update(anchor_move, 1)
        .unwrap();
    let transition = completed
        .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap())
        .connection_timing_transition()
        .unwrap();
    assert!(!transition.host_parameters_changed());
}

#[test]
fn connection_update_handles_same_event_past_instant_and_procedure_collisions() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
    let updated = LeConnectionTiming::new(2, 1, 40, 3, 200).unwrap();
    let map = LeDataChannelMap::new([0x03, 0, 0, 0, 0]).unwrap();
    let mut connection =
        LePeripheralConnection::from_request(request, LeChannelSelectionAlgorithm::AlgorithmTwo);
    connection.event_counter = 0xfffa;
    let mut completed = complete_missed(connection);

    completed
        .schedule_connection_update(updated, 0xfffa)
        .unwrap();
    assert_eq!(completed.timing(), updated);
    assert_eq!(
        completed.connection_timing_transition().unwrap().instant(),
        0xfffa
    );
    assert_eq!(
        completed.schedule_connection_update(updated, 0xfffa),
        Err(LePeripheralConnectionUpdateError::ProcedureAlreadyPending)
    );
    assert_eq!(
        completed.schedule_channel_map_update(map, 0xfffa),
        Err(LePeripheralChannelMapUpdateError::IncompatibleProcedurePending)
    );

    let mut connection =
        LePeripheralConnection::from_request(request, LeChannelSelectionAlgorithm::AlgorithmTwo);
    connection.event_counter = 0xfffa;
    let mut completed = complete_missed(connection);
    assert_eq!(
        completed.schedule_connection_update(updated, 0x7ff9),
        Err(LePeripheralConnectionUpdateError::InstantPassed)
    );
    completed.schedule_channel_map_update(map, 0).unwrap();
    assert_eq!(
        completed.schedule_connection_update(updated, 1),
        Err(LePeripheralConnectionUpdateError::IncompatibleProcedurePending)
    );

    let mut connection =
        LePeripheralConnection::from_request(request, LeChannelSelectionAlgorithm::AlgorithmTwo);
    connection.event_counter = 0xfffa;
    let mut completed = complete_missed(connection);
    completed.schedule_connection_update(updated, 0).unwrap();
    assert_eq!(
        completed.schedule_channel_map_update(map, 1),
        Err(LePeripheralChannelMapUpdateError::IncompatibleProcedurePending)
    );
    assert_eq!(
        completed.schedule_connection_update(updated, 2),
        Err(LePeripheralConnectionUpdateError::ProcedureAlreadyPending)
    );
}
