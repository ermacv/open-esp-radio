use super::super::test_support::{BSSID, LOCAL, authentication_response, deauthentication};
use super::*;
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_mac::station::StaDisconnectKind;

fn runtime() -> StaAuthenticationRuntime {
    StaAuthenticationRuntime::new(
        OwnAddress::new(IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
    )
}

#[test]
fn authentication_retries_own_the_sequence_and_stop_at_the_attempt_limit() {
    let mut ready = runtime();
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0x0ffe).unwrap());
    for ordinal in 1..=STA_AUTHENTICATION_ATTEMPT_LIMIT {
        let (mut active, attempt) = ready.begin_attempt(&mut sequence);
        assert_eq!(attempt.ordinal, ordinal);
        assert_eq!(
            attempt.sequence_number,
            SequenceNumber::new(0x0ffd).unwrap().wrapping_add(ordinal)
        );
        assert_eq!(attempt.response_timeout, STA_RESPONSE_TIMEOUT);
        active.observe_received_frame();
        match active.response_timed_out() {
            StaAuthenticationEvent::Retry {
                runtime,
                attempt,
                failure,
                received_frames,
                total_received_frames,
            } => {
                assert!(ordinal < STA_AUTHENTICATION_ATTEMPT_LIMIT);
                assert_eq!(attempt, ordinal);
                assert_eq!(failure, StaAuthenticationFailure::Timeout);
                assert_eq!(received_frames, 1);
                assert_eq!(total_received_frames, u32::from(ordinal));
                ready = runtime;
            }
            StaAuthenticationEvent::Failed {
                attempts,
                failure,
                total_received_frames,
            } => {
                assert_eq!(ordinal, STA_AUTHENTICATION_ATTEMPT_LIMIT);
                assert_eq!(attempts, ordinal);
                assert_eq!(failure, StaAuthenticationFailure::Timeout);
                assert_eq!(total_received_frames, u32::from(ordinal));
                return;
            }
            other => panic!("unexpected timeout {other:?}"),
        }
    }
    panic!("last attempt must end the runtime");
}

#[test]
fn irrelevant_management_returns_the_same_active_exchange() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());
    let (mut active, attempt) = runtime().begin_attempt(&mut sequence);
    active.observe_received_frame();
    let StaAuthenticationEvent::Irrelevant(mut active) = active.observe_management_frame(&[0; 30])
    else {
        panic!("irrelevant frame")
    };
    assert_eq!(active.active_received_frames(), 1);
    active.observe_received_frame();
    assert_eq!(active.total_received_frames(), 2);
    let StaAuthenticationEvent::Authenticated {
        attempt: completed,
        total_received_frames,
    } = active.observe_management_frame(&authentication_response(0))
    else {
        panic!("selected peer authenticates")
    };
    assert_eq!(completed, attempt.ordinal);
    assert_eq!(total_received_frames, 2);
}

#[test]
fn peer_disconnect_returns_a_retry_owner_but_rejection_consumes_the_epoch() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let (mut active, _) = runtime().begin_attempt(&mut sequence);
    active.observe_received_frame();
    let StaAuthenticationEvent::Retry {
        runtime,
        attempt,
        failure,
        received_frames,
        total_received_frames,
    } = active.observe_management_frame(&deauthentication(1))
    else {
        panic!("retry")
    };
    assert_eq!(attempt, 1);
    assert_eq!(
        failure,
        StaAuthenticationFailure::PeerDisconnect(StaDisconnect {
            kind: StaDisconnectKind::Deauthentication,
            reason_code: 1
        })
    );
    assert_eq!(received_frames, 1);
    assert_eq!(total_received_frames, 1);
    let (mut active, attempt) = runtime.begin_attempt(&mut sequence);
    assert_eq!(attempt.ordinal, 2);
    active.observe_received_frame();
    assert!(matches!(
        active.observe_management_frame(&authentication_response(17)),
        StaAuthenticationEvent::Failed {
            attempts: 2,
            failure: StaAuthenticationFailure::Rejected { status_code: 17 },
            total_received_frames: 2
        }
    ));
}
