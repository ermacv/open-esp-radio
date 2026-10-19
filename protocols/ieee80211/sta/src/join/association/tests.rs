use super::super::test_support::{BSSID, LOCAL, association_response, deauthentication};
use super::*;
use oer_ieee80211_mac::station::StaDisconnectKind;
use oer_time::{Duration, Instant};

fn at(millis: u64) -> Instant {
    Instant::from_micros(millis * 1_000)
}

fn ms(millis: u64) -> Duration {
    Duration::from_micros(millis * 1_000)
}

fn begin(now: Instant) -> Result<StaAssociationRuntime, StaAssociationFailure> {
    StaAssociationRuntime::new(
        OwnAddress::new(oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap()).unwrap(),
        LinkProtection::Ccmp,
        now,
    )
}

fn transmit(
    runtime: StaAssociationRuntime,
    now: Instant,
    sequence: &mut StaSequenceCounter,
) -> (StaAssociationRuntime, StaAssociationAttempt) {
    let StaAssociationPoll::Transmit { runtime, attempt } = runtime.poll(now, sequence) else {
        panic!("the request is due");
    };
    (runtime, attempt)
}

fn continuing(event: StaAssociationEvent) -> StaAssociationRuntime {
    let StaAssociationEvent::Irrelevant(runtime) = event else {
        panic!("the association continues: {event:?}");
    };
    runtime
}

fn failed(event: StaAssociationEvent, expected: StaAssociationFailure, received: u32) {
    let StaAssociationEvent::Failed {
        failure,
        total_received_frames,
    } = event
    else {
        panic!("expected a failure terminal: {event:?}");
    };
    assert_eq!(failure, expected);
    assert_eq!(total_received_frames, received);
}

#[test]
fn association_retry_schedule_is_finite_inside_vendor_deadline() {
    for ordinal in 1..=7 {
        assert_eq!(
            StaAssociationRetrySchedule::offset(ordinal),
            Some(ms(u64::from(ordinal - 1) * 160))
        );
    }
    assert_eq!(StaAssociationRetrySchedule::offset(0), None);
    assert_eq!(StaAssociationRetrySchedule::offset(8), None);
}

#[test]
fn association_runtime_owns_epoch_schedule_sequence_and_timeout() {
    let mut runtime = begin(at(0)).unwrap();
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0x0ffc).unwrap());
    let mut attempts = std::vec::Vec::new();
    let mut timed_out_at = None;

    for millis in 0..=1_000 {
        runtime = match runtime.poll(at(millis), &mut sequence) {
            StaAssociationPoll::Idle(runtime) => runtime,
            StaAssociationPoll::Transmit { runtime, attempt } => {
                attempts.push((millis, attempt));
                runtime
            }
            StaAssociationPoll::Failed {
                failure,
                total_received_frames,
            } => {
                assert_eq!(failure, StaAssociationFailure::Timeout);
                assert_eq!(total_received_frames, 1_000);
                timed_out_at = Some(millis);
                break;
            }
        };
        runtime.observe_received_frame();
    }

    assert_eq!(timed_out_at, Some(1_000));
    assert_eq!(attempts.len(), 7);
    for (index, (millis, attempt)) in attempts.into_iter().enumerate() {
        assert_eq!(millis, index as u64 * 160);
        assert_eq!(attempt.ordinal, index as u16 + 1);
        assert_eq!(attempt.offset, ms(index as u64 * 160));
        assert_eq!(
            attempt.sequence_number,
            SequenceNumber::new(0x0ffc)
                .unwrap()
                .wrapping_add(index as u16)
        );
    }
    // There is no runtime owner after the failure terminal. A UI test checks
    // that a second poll cannot be expressed, including after a successful RX.
}

#[test]
fn the_next_deadline_is_the_next_request_or_the_epoch_deadline() {
    let runtime = begin(at(5)).unwrap();
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    assert_eq!(runtime.next_deadline(), at(5));
    let (runtime, _) = transmit(runtime, at(5), &mut sequence);
    assert_eq!(runtime.next_deadline(), at(165));
    let StaAssociationPoll::Idle(mut runtime) = runtime.poll(at(164), &mut sequence) else {
        panic!("no request before its deadline");
    };
    for request in 1..7 {
        (runtime, _) = transmit(runtime, at(5 + request * 160), &mut sequence);
    }
    assert_eq!(runtime.next_deadline(), at(1_005));
}

#[test]
fn association_runtime_accepts_only_selected_peer_response() {
    let runtime = begin(at(0)).unwrap();
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());
    let (mut runtime, first) = transmit(runtime, at(0), &mut sequence);
    assert_eq!(first.ordinal, 1);
    runtime.observe_received_frame();
    let mut other_peer = association_response(0);
    other_peer[10] ^= 1;
    let runtime = continuing(runtime.observe_management_frame(&other_peer, at(1)));
    let event = runtime.observe_management_frame(&association_response(0), at(1));
    let StaAssociationEvent::Associated {
        response,
        total_received_frames,
    } = event
    else {
        panic!("selected peer succeeds: {event:?}");
    };
    assert_eq!(
        response,
        AssociationResponse {
            capability_info: 0x0431,
            status_code: 0,
            association_id: 42,
            ht_capability: false,
            he_capability: false,
            he_operation: false,
            wmm: false,
            wmm_parameters: None,
            association_comeback_tu: None,
        }
    );
    assert_eq!(total_received_frames, 1);
}

#[test]
fn association_runtime_reports_peer_disconnect_and_rejection() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let (mut disconnected, _) = transmit(begin(at(0)).unwrap(), at(0), &mut sequence);
    disconnected.observe_received_frame();
    failed(
        disconnected.observe_management_frame(&deauthentication(7), at(0)),
        StaAssociationFailure::PeerDisconnect(StaDisconnect {
            kind: StaDisconnectKind::Deauthentication,
            reason_code: 7,
        }),
        1,
    );
    let (rejected, _) = transmit(begin(at(0)).unwrap(), at(0), &mut sequence);
    failed(
        rejected.observe_management_frame(&association_response(17), at(0)),
        StaAssociationFailure::Rejected { status_code: 17 },
        0,
    );
}

fn refused_temporarily(comeback_tu: u32) -> std::vec::Vec<u8> {
    let mut frame = association_response(30).to_vec();
    frame.extend_from_slice(&[56, 5, 3]);
    frame.extend_from_slice(&comeback_tu.to_le_bytes());
    frame
}

#[test]
fn a_temporary_refusal_comes_back_once_after_its_comeback_time() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let (runtime, first) = transmit(begin(at(0)).unwrap(), at(0), &mut sequence);
    assert_eq!(first.ordinal, 1);
    let mut runtime =
        continuing(runtime.observe_management_frame(&refused_temporarily(1_000), at(0)));
    // (1000 + 100) TUs of 1024 us: 1126.4 ms, met by the first millisecond
    // poll at or after it. Poll every prior millisecond to prove no early TX.
    assert_eq!(runtime.next_deadline(), Instant::from_micros(1_126_400));
    for millis in 1..=1_126 {
        let StaAssociationPoll::Idle(next) = runtime.poll(at(millis), &mut sequence) else {
            panic!("comeback has not elapsed");
        };
        runtime = next;
    }
    let (runtime, again) = transmit(runtime, at(1_127), &mut sequence);
    assert_eq!(again.ordinal, 1);
    assert_eq!(again.offset, Duration::ZERO);
    assert_eq!(again.sequence_number, first.sequence_number.wrapping_add(1));
    failed(
        runtime.observe_management_frame(&refused_temporarily(1_000), at(1_127)),
        StaAssociationFailure::ComebackRefused { comeback_tu: 1_000 },
        0,
    );
}

#[test]
fn a_long_or_unnamed_temporary_refusal_ends_the_association() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let (long, _) = transmit(begin(at(0)).unwrap(), at(0), &mut sequence);
    failed(
        long.observe_management_frame(&refused_temporarily(5_001), at(0)),
        StaAssociationFailure::ComebackRefused { comeback_tu: 5_001 },
        0,
    );
    let (unnamed, _) = transmit(begin(at(0)).unwrap(), at(0), &mut sequence);
    failed(
        unnamed.observe_management_frame(&association_response(30), at(0)),
        StaAssociationFailure::Rejected { status_code: 30 },
        0,
    );
}

#[test]
fn association_and_comeback_require_the_complete_response_deadline() {
    let last_start = Instant::from_micros(u64::MAX - STA_RESPONSE_TIMEOUT.as_micros());
    let runtime = begin(last_start).unwrap();
    assert_eq!(runtime.response_deadline(), Instant::from_micros(u64::MAX));
    assert!(matches!(
        begin(last_start.checked_add(Duration::from_micros(1)).unwrap()),
        Err(StaAssociationFailure::DeadlineOverflow)
    ));
    // One case cannot fit the comeback itself; another fits the comeback,
    // but cannot fit its new response epoch. Both end explicitly.
    for now in [last_start, Instant::from_micros(u64::MAX - 6_000_000)] {
        let mut runtime = begin(now).unwrap();
        runtime.observe_received_frame();
        failed(
            runtime.observe_management_frame(&refused_temporarily(5_000), now),
            StaAssociationFailure::DeadlineOverflow,
            1,
        );
    }
}

#[test]
fn disconnect_during_comeback_consumes_the_owner_and_keeps_accounting() {
    let mut runtime = begin(at(0)).unwrap();
    runtime.observe_received_frame();
    let mut runtime =
        continuing(runtime.observe_management_frame(&refused_temporarily(100), at(1)));
    runtime.observe_received_frame();
    failed(
        runtime.observe_management_frame(&deauthentication(8), at(2)),
        StaAssociationFailure::PeerDisconnect(StaDisconnect {
            kind: StaDisconnectKind::Deauthentication,
            reason_code: 8,
        }),
        2,
    );
}
