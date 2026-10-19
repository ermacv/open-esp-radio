use super::*;
use oer_ieee80211_mac::ftm::{
    FTM_MEASUREMENT_PREFIX_LEN, FTM_PARAMETERS_ELEMENT_LEN, FtmMeasurementFields,
    encode_measurement,
};

#[test]
fn equal_numeric_generations_never_route_a_completion_into_another_requester() {
    let make = |peer| {
        let session = FtmRequester::<1>::new(config(2, 1))
            .start(peer, Instant::from_micros(0))
            .unwrap();
        let FtmRequesterService::Transmit(transmission) =
            session.service(Instant::from_micros(0)).unwrap()
        else {
            panic!()
        };
        transmission
    };
    let first = make(individual(1));
    let second = make(individual(2));
    assert_eq!(first.session_generation(), second.session_generation());
    assert_eq!(
        first.transmission_generation(),
        second.transmission_generation()
    );
    assert_eq!(first.body(), second.body());
    let (session, _) = first.complete(true, Instant::from_micros(10)).unwrap();
    assert_eq!(session.operation().peer(), individual(1));
    let (second_terminal, _) = second.reject_hardware_admission();
    let (_, FtmOutcome::Failed { partial, reason }) = second_terminal.into_parts() else {
        panic!()
    };
    assert_eq!(partial.operation.peer(), individual(2));
    assert_eq!(reason, FtmSessionFailure::HardwareAdmissionRejected);
    assert_eq!(session.next_deadline(), Some(Instant::from_micros(1_010)));
}

#[test]
fn cancellation_failure_and_timeout_return_samples_and_unmatched_local_timing() {
    for ending in 0..3 {
        let peer = individual(7);
        let mut session = start_published(FtmRequester::<3>::new(config(4, 1)), peer);
        let initial = initial_body(1, 4);
        session
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                Instant::from_micros(30),
            )
            .unwrap();
        let follow_up = follow_up_body(2, 1, 80, 160);
        let pending = FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap();
        session
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&follow_up).unwrap(),
                Some(pending),
                Instant::from_micros(40),
            )
            .unwrap();
        let terminal = match ending {
            0 => session.cancel(),
            1 => {
                let invalid = follow_up_body(4, 2, 180, 260);
                assert_eq!(
                    session.on_measurement(
                        peer,
                        FtmMeasurement::decode_body(&invalid).unwrap(),
                        Some(FtmLocalExchangeTiming::new(4, ps(300), ps(320)).unwrap()),
                        Instant::from_micros(50)
                    ),
                    Err(FtmRequesterError::DialogTokenOutOfSequence {
                        expected: 3,
                        actual: 4
                    })
                );
                session.cancel()
            }
            _ => {
                let deadline = session.next_deadline().unwrap();
                let FtmRequesterService::Finished(terminal) = session.service(deadline).unwrap()
                else {
                    panic!("deadline produces terminal")
                };
                terminal
            }
        };
        let (requester, outcome) = terminal.into_parts();
        let partial = match (ending, outcome) {
            (0, FtmOutcome::Cancelled(partial))
            | (
                1,
                FtmOutcome::Failed {
                    reason: FtmSessionFailure::ProtocolViolation,
                    partial,
                },
            )
            | (
                2,
                FtmOutcome::Failed {
                    reason: FtmSessionFailure::SessionTimedOut,
                    partial,
                },
            ) => partial,
            _ => panic!("exact cancellation/failure reason must survive"),
        };
        assert_eq!(partial.operation.peer(), peer);
        assert_eq!(partial.sample_count(), 1);
        assert_eq!(partial.pending, Some(pending));
        let sample = partial.sample(0).unwrap();
        assert_eq!(sample.dialog_token, 1);
        let next = requester
            .start(individual(8), Instant::from_micros(20_000))
            .unwrap();
        assert_eq!(next.operation().session_generation(), 2);
        assert_eq!(partial.sample(0), Some(sample));
    }
}

#[test]
fn cancel_after_rx_completed_delivers_the_unconsumed_result() {
    let peer = individual(9);
    let mut session = start_published(FtmRequester::<1>::new(config(2, 1)), peer);
    let initial = initial_body(1, 2);
    session
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            Instant::from_micros(30),
        )
        .unwrap();
    let final_body = follow_up_body(0, 1, 80, 160);
    assert_eq!(
        session.on_measurement(
            peer,
            FtmMeasurement::decode_body(&final_body).unwrap(),
            None,
            Instant::from_micros(40)
        ),
        Ok(FtmMeasurementDisposition::Complete { samples: 1 })
    );
    let (requester, FtmOutcome::Complete(result)) = session.cancel().into_parts() else {
        panic!("cancellation must not erase a pending result")
    };
    assert_eq!(result.sample_count(), 1);
    assert_eq!(result.sample(0).unwrap().dialog_token, 1);
    assert!(
        requester
            .start(individual(10), Instant::from_micros(50))
            .is_ok()
    );
}

fn ps(value: u64) -> FtmTimestampPs {
    FtmTimestampPs::new(value).unwrap()
}

fn request_parameters(count: u8) -> FtmRequestParameters {
    FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        count,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap()
}

fn config(count: u8, attempts: u8) -> FtmRequesterConfig {
    FtmRequesterConfig::new(
        request_parameters(count),
        oer_time::Duration::from_micros(1_000),
        oer_time::Duration::from_micros(100),
        oer_time::Duration::from_micros(10_000),
        attempts,
    )
    .unwrap()
}

fn response(count: u8) -> FtmResponseParameters {
    FtmResponseParameters {
        status: FtmResponseStatus::Success,
        number_of_bursts_exponent: 0,
        burst_duration: FtmBurstDuration::Millis8,
        min_delta_ftm_100us: 2,
        partial_tsf_timer: 10,
        asap_capable: true,
        asap: true,
        ftms_per_burst: count,
        format_and_bandwidth: FtmFormatAndBandwidth::HtMixed20Mhz,
        burst_period_100ms: 0,
    }
}

fn measurement_body<const N: usize>(
    fields: FtmMeasurementFields,
    parameters: Option<FtmResponseParameters>,
) -> [u8; N] {
    let mut body = [0_u8; N];
    let element = parameters.map(|parameters| parameters.encode_element().unwrap());
    let information_elements = element.as_ref().map_or(&[][..], |element| &element[..]);
    assert_eq!(
        encode_measurement(fields, information_elements, &mut body),
        Ok(N)
    );
    body
}

fn initial_body(
    token: u8,
    count: u8,
) -> [u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN] {
    measurement_body(
        FtmMeasurementFields {
            dialog_token: token,
            follow_up_dialog_token: 0,
            tod: FtmTimestampPs::ZERO,
            toa: FtmTimestampPs::ZERO,
            tod_error: FtmTodError {
                max_error_exponent: 0,
                not_continuous: false,
            },
            toa_error: FtmToaError {
                max_error_exponent: 0,
            },
        },
        Some(response(count)),
    )
}

fn follow_up_body(
    dialog_token: u8,
    follow_up_dialog_token: u8,
    t1: u64,
    t4: u64,
) -> [u8; FTM_MEASUREMENT_PREFIX_LEN] {
    measurement_body(
        FtmMeasurementFields {
            dialog_token,
            follow_up_dialog_token,
            tod: ps(t1),
            toa: ps(t4),
            tod_error: FtmTodError {
                max_error_exponent: 2,
                not_continuous: false,
            },
            toa_error: FtmToaError {
                max_error_exponent: 3,
            },
        },
        None,
    )
}

fn start_published<const N: usize>(
    requester: FtmRequester<N>,
    peer: IndividualAddress,
) -> FtmSession<N> {
    let session = requester.start(peer, Instant::from_micros(10)).unwrap();
    let FtmRequesterService::Transmit(transmission) =
        session.service(Instant::from_micros(10)).unwrap()
    else {
        panic!("request must be ready")
    };
    assert_eq!(transmission.body()[..3], [4, 32, 1]);
    transmission
        .complete(true, Instant::from_micros(20))
        .unwrap()
        .0
}

fn finish<const N: usize>(session: FtmSession<N>) -> (FtmRequester<N>, FtmSessionResult<N>) {
    let FtmRequesterService::Finished(terminal) =
        session.service(Instant::from_micros(50)).unwrap()
    else {
        panic!("measurement already completed")
    };
    let (requester, FtmOutcome::Complete(result)) = terminal.into_parts() else {
        panic!("complete result")
    };
    (requester, result)
}

fn failure<const N: usize>(session: FtmSession<N>) -> FtmSessionFailure {
    let (_, FtmOutcome::Failed { reason, .. }) = session.cancel().into_parts() else {
        panic!("session already failed")
    };
    reason
}

#[test]
fn config_rejects_unbounded_or_scheduled_profiles() {
    let multiple = FtmRequestParameters::new(
        1,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        4,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        1,
    )
    .unwrap();
    assert_eq!(
        FtmRequesterConfig::new(
            multiple,
            oer_time::Duration::from_micros(1),
            oer_time::Duration::from_micros(1),
            oer_time::Duration::from_micros(1),
            1
        ),
        Err(FtmRequesterConfigError::MultipleBurstsUnsupported)
    );
    let unbounded = FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        0,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap();
    assert_eq!(
        FtmRequesterConfig::new(
            unbounded,
            oer_time::Duration::from_micros(1),
            oer_time::Duration::from_micros(1),
            oer_time::Duration::from_micros(1),
            1
        ),
        Err(FtmRequesterConfigError::NoMeasurementCountPreferenceUnsupported)
    );
}

#[test]
fn request_retries_transfer_the_only_session_owner() {
    let session = FtmRequester::<2>::new(config(2, 2))
        .start(individual(1), Instant::from_micros(0))
        .unwrap();
    let refused = session.refuse_request(71);
    assert_eq!(refused.request, 71);
    assert_eq!(
        refused.wait,
        FtmWaitSource::SessionTerminal(session.operation())
    );
    let FtmRequesterService::Transmit(first) = session.service(Instant::from_micros(0)).unwrap()
    else {
        panic!()
    };
    let first_generation = first.transmission_generation();
    let (session, event) = first.complete(false, Instant::from_micros(10)).unwrap();
    assert_eq!(
        event,
        FtmRequesterEvent::RequestRetryScheduled {
            retry_at: Instant::from_micros(110)
        }
    );
    let FtmRequesterService::Wait(session) = session.service(Instant::from_micros(109)).unwrap()
    else {
        panic!("retry must wait")
    };
    let FtmRequesterService::Transmit(second) = session.service(Instant::from_micros(110)).unwrap()
    else {
        panic!("retry due")
    };
    assert_ne!(first_generation, second.transmission_generation());
    let (session, _) = second.complete(true, Instant::from_micros(111)).unwrap();
    assert_eq!(session.next_deadline(), Some(Instant::from_micros(1_111)));
}

#[test]
fn scheduling_overflow_returns_the_unchanged_requester_or_transmission() {
    let requester = FtmRequester::<1>::new(config(2, 2));
    let FtmRefusal {
        error,
        owner: requester,
    } = requester
        .start(individual(1), Instant::from_micros(u64::MAX))
        .err()
        .unwrap();
    assert_eq!(error, FtmRequesterError::DeadlineOverflow);
    let session = requester
        .start(individual(1), Instant::from_micros(0))
        .unwrap();
    assert_eq!(session.operation().session_generation(), 1);
    let FtmRequesterService::Transmit(transmission) =
        session.service(Instant::from_micros(0)).unwrap()
    else {
        panic!()
    };
    let body = *transmission.body();
    let FtmRefusal {
        error,
        owner: transmission,
    } = transmission
        .complete(true, Instant::from_micros(u64::MAX))
        .err()
        .unwrap();
    assert_eq!(error, FtmRequesterError::DeadlineOverflow);
    assert_eq!(transmission.body(), &body);
    assert_eq!(transmission.transmission_generation(), 1);
    let FtmRefusal {
        error,
        owner: transmission,
    } = transmission
        .complete(false, Instant::from_micros(u64::MAX))
        .err()
        .unwrap();
    assert_eq!(error, FtmRequesterError::DeadlineOverflow);
    let terminal = transmission.cancel();
    let (requester, FtmOutcome::Cancelled(partial)) = terminal.into_parts() else {
        panic!()
    };
    assert_eq!(partial.sample_count(), 0);
    assert_eq!(
        requester
            .start(individual(2), Instant::from_micros(0))
            .unwrap()
            .operation()
            .session_generation(),
        2
    );
}

#[test]
fn a_late_service_returns_the_queued_session_before_minting_a_transmission() {
    let session = FtmRequester::<1>::new(config(2, 2))
        .start(individual(1), Instant::from_micros(0))
        .unwrap();
    let operation = session.operation();
    let deadline = session.next_deadline();
    let FtmRefusal {
        error,
        owner: session,
    } = session
        .service(Instant::from_micros(u64::MAX))
        .err()
        .unwrap();
    assert_eq!(error, FtmRequesterError::DeadlineOverflow);
    assert_eq!(session.operation(), operation);
    assert_eq!(session.next_deadline(), deadline);
    let FtmRequesterService::Transmit(transmission) =
        session.service(Instant::from_micros(1)).unwrap()
    else {
        panic!("the queued request must remain available")
    };
    assert_eq!(transmission.transmission_generation(), 1);
    let (_, outcome) = transmission.cancel().into_parts();
    assert!(matches!(outcome, FtmOutcome::Cancelled(_)));
}

#[test]
fn three_ftm_frames_deliver_two_owned_samples_without_claiming_distance() {
    let peer = individual(0x22);
    let mut requester = start_published(FtmRequester::<2>::new(config(3, 1)), peer);

    let initial = initial_body(1, 3);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(1_100), ps(1_200)).unwrap()),
            oer_time::Instant::from_micros(30),
        ),
        Ok(FtmMeasurementDisposition::InitialAccepted {
            dialog_token: 1,
            allocated_ftms_per_burst: 3,
            maximum_samples: 2,
        })
    );

    let follow_up = follow_up_body(2, 1, 1_000, 1_400);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&follow_up).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(1_500), ps(1_600)).unwrap()),
            oer_time::Instant::from_micros(40),
        ),
        Ok(FtmMeasurementDisposition::SampleAccepted {
            dialog_token: 1,
            sample_index: 0,
        })
    );
    let terminal = follow_up_body(0, 2, 1_400, 1_800);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            oer_time::Instant::from_micros(50),
        ),
        Ok(FtmMeasurementDisposition::Complete { samples: 2 })
    );

    let (_requester, result) = finish(requester);
    assert_eq!(result.peer, peer);
    assert_eq!(result.sample_count(), 2);
    assert_eq!(
        result.sample(0).unwrap().raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 400,
            initiator_turnaround_ps: 100,
            difference_ps: 300,
        })
    );
    assert_eq!(
        result.sample(1).unwrap().raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 400,
            initiator_turnaround_ps: 100,
            difference_ps: 300,
        })
    );
    let next = _requester.start(peer, Instant::from_micros(60)).unwrap();
    let (_, FtmOutcome::Cancelled(empty)) = next.cancel().into_parts() else {
        panic!("the next session has no measurement")
    };
    assert_eq!(empty.sample_count(), 0);
    assert!(empty.sample(0).is_none());
    assert_eq!(result.sample_count(), 2);
    assert_eq!(result.sample(0).unwrap().dialog_token, 1);
    assert_eq!(result.sample(1).unwrap().dialog_token, 2);
    assert!(result.sample(2).is_none());
}

#[test]
fn ftm_retransmission_deduplicates_follow_up_but_owns_new_token() {
    let peer = individual(3);
    let mut requester = start_published(FtmRequester::<2>::new(config(3, 1)), peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        )
        .unwrap();
    let measured = follow_up_body(2, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&measured).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            oer_time::Instant::from_micros(40),
        )
        .unwrap();
    let retry = follow_up_body(3, 1, 80, 160);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&retry).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
            oer_time::Instant::from_micros(50),
        ),
        Ok(FtmMeasurementDisposition::DuplicateSample { dialog_token: 1 })
    );
    let terminal = follow_up_body(0, 3, 280, 360);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            oer_time::Instant::from_micros(60),
        )
        .unwrap();
    let (_requester, result) = finish(requester);
    assert_eq!(result.sample_count(), 2);
    assert!(result.sample(0).is_some());
    assert_eq!(result.sample(1).unwrap().dialog_token, 3);
}

#[test]
fn abandoned_retransmission_token_cannot_be_reused() {
    let peer = individual(4);
    let mut requester = start_published(FtmRequester::<3>::new(config(4, 1)), peer);
    let initial = initial_body(1, 4);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        )
        .unwrap();
    let measured = follow_up_body(2, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&measured).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            oer_time::Instant::from_micros(40),
        )
        .unwrap();
    let retry = follow_up_body(3, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&retry).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
            oer_time::Instant::from_micros(50),
        )
        .unwrap();
    let reused = follow_up_body(2, 3, 280, 360);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&reused).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(400), ps(420)).unwrap()),
            oer_time::Instant::from_micros(60),
        ),
        Err(FtmRequesterError::DialogTokenReused)
    );
    assert_eq!(failure(requester), FtmSessionFailure::ProtocolViolation);
}

#[test]
fn capacity_and_hardware_admission_fail_before_publication() {
    let requester = FtmRequester::<1>::new(config(2, 1));
    let session = requester
        .start(individual(1), Instant::from_micros(0))
        .unwrap();
    assert_eq!(session.operation().session_generation(), 1);
    assert!(matches!(
        session.service(Instant::from_micros(0)),
        Ok(FtmRequesterService::Transmit(_))
    ));
    let too_small = FtmRequester::<1>::new(config(3, 1));
    let refusal = too_small
        .start(individual(1), Instant::from_micros(0))
        .err()
        .unwrap();
    assert_eq!(
        refusal.error,
        FtmRequesterError::CapacityTooSmall {
            required_samples: 2,
            capacity: 1
        }
    );
    assert_eq!(refusal.owner.config(), config(3, 1));
    let session = FtmRequester::<2>::new(config(3, 1))
        .start(individual(1), Instant::from_micros(0))
        .unwrap();
    let FtmRequesterService::Transmit(transmission) =
        session.service(Instant::from_micros(0)).unwrap()
    else {
        panic!()
    };
    let (terminal, event) = transmission.reject_hardware_admission();
    assert_eq!(
        event,
        FtmRequesterEvent::Failed(FtmSessionFailure::HardwareAdmissionRejected)
    );
    let (requester, FtmOutcome::Failed { reason, partial }) = terminal.into_parts() else {
        panic!()
    };
    assert_eq!(reason, FtmSessionFailure::HardwareAdmissionRejected);
    assert_eq!(partial.sample_count(), 0);
    assert!(
        requester
            .start(individual(2), Instant::from_micros(0))
            .is_ok()
    );
}

#[test]
fn one_ftm_frame_is_a_terminal_initial_with_zero_samples() {
    let peer = individual(5);
    let mut requester = start_published(FtmRequester::<0>::new(config(1, 1)), peer);
    let terminal_initial = initial_body(0, 1);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal_initial).unwrap(),
            None,
            oer_time::Instant::from_micros(30),
        ),
        Ok(FtmMeasurementDisposition::Complete { samples: 0 })
    );
    let (_requester, result) = finish(requester);
    assert_eq!(result.negotiated.ftms_per_burst, 1);
    assert_eq!(result.sample_count(), 0);

    let mut nonterminal = start_published(FtmRequester::<0>::new(config(1, 1)), peer);
    let invalid = initial_body(1, 1);
    assert_eq!(
        nonterminal.on_measurement(
            peer,
            FtmMeasurement::decode_body(&invalid).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        ),
        Err(FtmRequesterError::TooManyMeasurements)
    );
    assert_eq!(nonterminal.owner.samples.len(), 0);
}

#[test]
fn initial_retransmission_replaces_timing_and_tokens_wrap_consecutively() {
    let peer = individual(6);
    let mut requester = start_published(FtmRequester::<2>::new(config(3, 1)), peer);
    let initial = initial_body(254, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(254, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        )
        .unwrap();

    let retransmission = initial_body(255, 3);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&retransmission).unwrap(),
            Some(FtmLocalExchangeTiming::new(255, ps(200), ps(220)).unwrap()),
            oer_time::Instant::from_micros(40),
        ),
        Ok(FtmMeasurementDisposition::InitialRetransmissionAccepted {
            abandoned_dialog_token: 254,
            dialog_token: 255,
        })
    );

    let follow_up = follow_up_body(1, 255, 180, 260);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&follow_up).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(300), ps(320)).unwrap()),
            oer_time::Instant::from_micros(50),
        ),
        Ok(FtmMeasurementDisposition::SampleAccepted {
            dialog_token: 255,
            sample_index: 0,
        })
    );
    let terminal = follow_up_body(0, 1, 280, 360);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            oer_time::Instant::from_micros(60),
        )
        .unwrap();
    let (_requester, result) = finish(requester);
    assert_eq!(result.sample_count(), 2);
    assert_eq!(result.sample(0).unwrap().dialog_token, 255);
    assert_eq!(result.sample(0).unwrap().initiator_arrival_t2_ps, ps(200));
}

#[test]
fn nonconsecutive_dialog_token_fails_before_sample_mutation() {
    let peer = individual(7);
    let mut requester = start_published(FtmRequester::<2>::new(config(3, 1)), peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        )
        .unwrap();
    let skipped = follow_up_body(3, 1, 80, 160);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&skipped).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(200), ps(220)).unwrap()),
            oer_time::Instant::from_micros(40),
        ),
        Err(FtmRequesterError::DialogTokenOutOfSequence {
            expected: 2,
            actual: 3,
        })
    );
    assert_eq!(requester.owner.samples.len(), 0);
    assert_eq!(failure(requester), FtmSessionFailure::ProtocolViolation);
}

#[test]
fn initial_retransmission_must_preserve_the_negotiated_body() {
    let peer = individual(8);
    let mut requester = start_published(FtmRequester::<2>::new(config(3, 1)), peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            oer_time::Instant::from_micros(30),
        )
        .unwrap();

    let changed = initial_body(2, 2);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&changed).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            oer_time::Instant::from_micros(40),
        ),
        Err(FtmRequesterError::InitialRetransmissionMismatch)
    );
    assert_eq!(requester.owner.samples.len(), 0);
    assert_eq!(failure(requester), FtmSessionFailure::ProtocolViolation);
}

#[test]
fn timestamp_wrap_is_bounded_to_half_range() {
    let exchange = FtmRawExchange {
        dialog_token: 1,
        responder_departure_t1_ps: ps(FTM_TIMESTAMP_MASK - 9),
        responder_ack_arrival_t4_ps: ps(20),
        initiator_arrival_t2_ps: ps(100),
        initiator_ack_departure_t3_ps: ps(110),
        tod_error: FtmTodError {
            max_error_exponent: 1,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 1,
        },
    };
    assert_eq!(
        exchange.raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 30,
            initiator_turnaround_ps: 10,
            difference_ps: 20,
        })
    );
    let ambiguous = FtmRawExchange {
        responder_ack_arrival_t4_ps: ps(FTM_TIMESTAMP_HALF_RANGE),
        responder_departure_t1_ps: ps(0),
        ..exchange
    };
    assert_eq!(
        ambiguous.raw_interval_difference(),
        Err(FtmTimestampArithmeticError::AmbiguousResponderWrap)
    );
}

#[test]
fn local_timing_proves_a_nonzero_token_through_the_bitmap_boundary() {
    assert_eq!(
        FtmLocalExchangeTiming::new(0, ps(100), ps(120)),
        Err(FtmRequesterError::ZeroLocalDialogToken),
    );
    let mut tokens = TokenSet::EMPTY;
    for token in [1, 31, 32, 254, 255] {
        let timing = FtmLocalExchangeTiming::new(token, ps(100), ps(120)).unwrap();
        assert_eq!(timing.dialog_token.get(), token);
        assert!(!tokens.contains(token));
        tokens.insert(timing.dialog_token);
        assert!(tokens.contains(token));
    }
    assert!(!tokens.contains(0));
}

fn individual(octet: u8) -> IndividualAddress {
    IndividualAddress::new([2, octet, octet, octet, octet, octet]).unwrap()
}
