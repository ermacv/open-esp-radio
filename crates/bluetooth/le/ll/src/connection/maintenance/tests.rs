use super::*;
use crate::connection::*;

fn connection(algorithm_two: bool, counter: u16) -> LePeripheralConnection {
    let request =
        LeLegacyConnectionRequest::decode(&super::super::tests::connection_request(algorithm_two))
            .unwrap();
    let mut connection = LePeripheralConnection::from_request(
        request,
        if algorithm_two {
            LeChannelSelectionAlgorithm::AlgorithmTwo
        } else {
            LeChannelSelectionAlgorithm::AlgorithmOne
        },
    );
    connection.event_counter = counter;
    connection
}

fn served(connection: LePeripheralConnection) -> LePeripheralConnectionEventCompleted {
    let mut completed = connection
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
    completed.observe_valid_packet_header(0x05); // Empty Data PDU, NESN=1, SN=0.
    completed
}

fn next(
    completed: LePeripheralConnectionEventCompleted,
    activity: LePeripheralConnectionEventPeerActivity,
) -> LePeripheralConnectionEventCompleted {
    completed
        .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap())
        .commit()
        .into_submitted()
        .complete(activity)
}

#[test]
fn initial_rx_is_not_initial_acknowledgement() {
    let missed = connection(false, 0)
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    assert_eq!(
        missed
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Err(SkipBlocked::Establishment)
    );
    let mut received = next(missed, LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(
        received
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Err(SkipBlocked::InitialAcknowledgement)
    );
    received.observe_valid_packet_header(0x01);
    assert_eq!(
        received
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Err(SkipBlocked::InitialAcknowledgement)
    );
    received.observe_valid_packet_header(0x05);
    assert_eq!(
        received
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Ok(())
    );
}

#[test]
fn deliberate_skip_preserves_channel_counter_and_supervision_timeline() {
    for algorithm_two in [false, true] {
        for counter in [0, 100, u16::MAX - 1, u16::MAX] {
            let intentional = served(connection(algorithm_two, counter));
            let original_state = intentional.connection_state();
            let candidate = intentional
                .prepare_maintenance_event(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap(),
                )
                .unwrap();
            let control = served(connection(algorithm_two, counter));
            let control = next(control, LePeripheralConnectionEventPeerActivity::Missed)
                .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());
            assert_eq!(candidate.delta().skipped(), 1);
            assert_eq!(candidate.event_counter(), counter.wrapping_add(2));
            assert_eq!(candidate.channel(), control.channel());
            assert_eq!(candidate.connection_state(), original_state);
            assert_eq!(
                candidate.commit().event_counter(),
                control.commit().event_counter()
            );
        }
    }
}

#[test]
fn cancelled_preview_returns_same_owner_and_does_not_spend_skip_budget() {
    let completed = served(connection(true, 10));
    let expected = served(connection(true, 10));
    let returned = completed
        .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
        .unwrap()
        .cancel();
    assert_eq!(returned, expected);
    assert!(
        returned
            .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
            .is_ok()
    );
}

#[test]
fn missed_events_cannot_release_the_pause_recovery_obligation() {
    let completed = served(connection(true, 0))
        .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
        .unwrap()
        .commit()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed);
    let (error, mut completed) = completed
        .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
        .unwrap_err();
    assert_eq!(error, SkipBlocked::RecoveryEventRequired);
    for _ in 0..3 {
        completed = next(completed, LePeripheralConnectionEventPeerActivity::Missed);
        assert_eq!(
            completed.maintenance_skip_eligible(
                LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
            ),
            Err(SkipBlocked::RecoveryEventRequired)
        );
    }
    completed = next(completed, LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(
        completed
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Err(SkipBlocked::RecoveryEventRequired),
        "an RX completion before CRC/MIC validation does not release recovery",
    );
    completed.observe_valid_packet_header(0x05);
    assert_eq!(
        completed
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(1).unwrap()),
        Ok(())
    );
    assert!(
        completed
            .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
            .is_ok()
    );
}

#[test]
fn instant_needs_peer_ack_evidence_and_protects_the_two_boundary_events() {
    for counter in [10, u16::MAX - 2] {
        for channel_map in [false, true] {
            let mut completed = served(connection(true, counter));
            let instant = counter.wrapping_add(4);
            if channel_map {
                completed
                    .schedule_channel_map_update(LeDataChannelMap::all(), instant)
                    .unwrap();
            } else {
                completed
                    .schedule_connection_update(completed.timing(), instant)
                    .unwrap();
            }
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Err(SkipBlocked::InstantAcknowledgement)
            );
            completed.observe_valid_packet_header(0x05);
            completed.observe_valid_packet_header(0x05); // Retransmission is not ACK evidence.
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Err(SkipBlocked::InstantAcknowledgement)
            );
            completed.observe_valid_packet_header(0x0d); // Next peer SN proves acknowledgement.
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Ok(())
            );
            completed = next(completed, LePeripheralConnectionEventPeerActivity::Observed);
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Ok(())
            );
            completed = next(completed, LePeripheralConnectionEventPeerActivity::Observed);
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Err(SkipBlocked::InstantProcedure)
            );
            completed = next(completed, LePeripheralConnectionEventPeerActivity::Observed);
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Err(SkipBlocked::InstantProcedure)
            );
            completed = next(completed, LePeripheralConnectionEventPeerActivity::Observed);
            assert_eq!(completed.event_counter(), instant);
            assert_eq!(
                completed.maintenance_skip_eligible(
                    LePeripheralConnectionEventDelta::from_skipped(1).unwrap()
                ),
                Ok(())
            );
        }
    }
}

#[test]
fn rejected_skip_returns_the_pending_instant_unchanged() {
    let mut original = served(connection(false, 0));
    let timing = LeConnectionTiming::new(2, 1, 40, 0, 200).unwrap();
    original.schedule_connection_update(timing, 2).unwrap();
    let (error, returned) = original
        .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(1).unwrap())
        .unwrap_err();
    assert_eq!(error, SkipBlocked::InstantProcedure);
    let next = next(returned, LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(next.event_counter(), 1);
    let at_instant =
        next.prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());
    assert_eq!(
        at_instant.connection_timing_transition().unwrap().instant(),
        2
    );
    assert_eq!(at_instant.timing(), timing);
}

#[test]
fn budgeted_pause_matches_each_omitted_event_for_both_channel_algorithms() {
    for algorithm_two in [false, true] {
        for counter in [0, 37, u16::MAX - 2, u16::MAX] {
            for skipped in [2, 3, 9, 100] {
                let delta = LePeripheralConnectionEventDelta::from_skipped(skipped).unwrap();
                let candidate = served(connection(algorithm_two, counter))
                    .prepare_maintenance_event(delta)
                    .unwrap();
                let mut ordinary = served(connection(algorithm_two, counter));
                for _ in 0..skipped {
                    ordinary = next(ordinary, LePeripheralConnectionEventPeerActivity::Missed);
                }
                let ordinary = ordinary
                    .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());
                assert_eq!(candidate.channel(), ordinary.channel());
                assert_eq!(candidate.event_counter(), counter.wrapping_add(skipped + 1));
                assert_eq!(candidate.event_counter(), ordinary.event_counter());
                let mut completed = candidate
                    .commit()
                    .into_submitted()
                    .complete(LePeripheralConnectionEventPeerActivity::Observed);
                assert_eq!(
                    completed.maintenance_skip_eligible(delta),
                    Err(SkipBlocked::RecoveryEventRequired)
                );
                completed.observe_valid_packet_header(0x05);
                assert_eq!(completed.maintenance_skip_eligible(delta), Ok(()));
            }
        }
    }
}

#[test]
fn pause_checks_the_entire_omitted_range_against_both_instants() {
    for (counter, channel_map) in [10, u16::MAX - 3]
        .into_iter()
        .flat_map(|counter| [false, true].map(|map| (counter, map)))
    {
        let mut completed = served(connection(true, counter));
        if channel_map {
            completed
                .schedule_channel_map_update(LeDataChannelMap::all(), counter.wrapping_add(5))
                .unwrap();
        } else {
            completed
                .schedule_connection_update(completed.timing(), counter.wrapping_add(5))
                .unwrap();
        }
        completed.observe_valid_packet_header(0x05);
        completed.observe_valid_packet_header(0x0d);
        // A pause cannot jump across the protected predecessor or Instant.
        for skipped in 1..=14 {
            let delta = LePeripheralConnectionEventDelta::from_skipped(skipped).unwrap();
            assert_eq!(
                completed.maintenance_skip_eligible(delta),
                if skipped < 4 {
                    Ok(())
                } else {
                    Err(SkipBlocked::InstantProcedure)
                }
            );
        }
        let delta = LePeripheralConnectionEventDelta::from_skipped(3).unwrap();
        let cancelled = completed.prepare_maintenance_event(delta).unwrap().cancel();
        assert_eq!(cancelled.maintenance_skip_eligible(delta), Ok(()));
        let (_, returned) = cancelled
            .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(8).unwrap())
            .unwrap_err();
        assert_eq!(returned.maintenance_skip_eligible(delta), Ok(()));
    }
}

#[test]
fn maintenance_rejects_empty_or_ambiguous_counter_ranges_without_consuming_owner() {
    let mut completed = served(connection(true, 100));
    for distance in [1, 0x8000, u16::MAX] {
        let delta = LePeripheralConnectionEventDelta::new(distance).unwrap();
        let (reason, returned) = completed.prepare_maintenance_event(delta).unwrap_err();
        assert_eq!(reason, SkipBlocked::InvalidPause);
        completed = returned;
    }
    assert!(
        completed
            .prepare_maintenance_event(LePeripheralConnectionEventDelta::from_skipped(3).unwrap())
            .is_ok()
    );
}

#[test]
fn valid_plaintext_receive_without_payload_releases_only_recovery() {
    let delta = LePeripheralConnectionEventDelta::from_skipped(3).unwrap();
    let mut completed = served(connection(true, 4))
        .prepare_maintenance_event(delta)
        .unwrap()
        .commit()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
    assert_eq!(
        completed.maintenance_skip_eligible(delta),
        Err(SkipBlocked::RecoveryEventRequired)
    );
    completed.observe_valid_plaintext_reception();
    assert_eq!(completed.maintenance_skip_eligible(delta), Ok(()));
    let mut initial = connection(true, 0)
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed);
    initial.observe_valid_plaintext_reception();
    assert_eq!(
        initial.maintenance_skip_eligible(delta),
        Err(SkipBlocked::InitialAcknowledgement)
    );
}

#[test]
fn plaintext_receive_without_header_cannot_confirm_an_instant_indication() {
    let mut completed = served(connection(true, 3));
    completed
        .schedule_channel_map_update(LeDataChannelMap::all(), 20)
        .unwrap();
    completed.observe_valid_plaintext_reception();
    assert_eq!(
        completed
            .maintenance_skip_eligible(LePeripheralConnectionEventDelta::from_skipped(3).unwrap()),
        Err(SkipBlocked::InstantAcknowledgement)
    );
}
