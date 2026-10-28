use super::super::test_support::{BSSID, LOCAL, authentication_response, deauthentication};
use super::*;
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::station::StaDisconnectKind;

fn runtime() -> StaAuthenticationRuntime {
    StaAuthenticationRuntime::new(
        OwnAddress::new(IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
    )
}

#[test]
fn authentication_retries_own_the_sequence_and_stop_at_the_attempt_limit() {
    let mut ready = runtime();
    let mut sequence = SequenceCounter::new(SequenceNumber::new(0x0ffe).unwrap());
    for ordinal in 1..=STA_AUTHENTICATION_ATTEMPT_LIMIT {
        let (active, attempt) = ready.begin_attempt(&mut sequence);
        assert_eq!(attempt.ordinal, ordinal);
        assert_eq!(
            attempt.sequence_number,
            SequenceNumber::new(0x0ffd).unwrap().wrapping_add(ordinal)
        );
        assert_eq!(attempt.response_timeout, STA_RESPONSE_TIMEOUT);
        match active.response_timed_out() {
            StaAuthenticationEvent::Retry {
                runtime,
                attempt,
                failure,
            } => {
                assert!(ordinal < STA_AUTHENTICATION_ATTEMPT_LIMIT);
                assert_eq!(attempt, ordinal);
                assert_eq!(failure, StaAuthenticationFailure::Timeout);
                ready = runtime;
            }
            StaAuthenticationEvent::Failed { attempts, failure } => {
                assert_eq!(ordinal, STA_AUTHENTICATION_ATTEMPT_LIMIT);
                assert_eq!(attempts, ordinal);
                assert_eq!(failure, StaAuthenticationFailure::Timeout);
                return;
            }
            other => panic!("unexpected timeout {other:?}"),
        }
    }
    panic!("last attempt must end the runtime");
}

#[test]
fn irrelevant_management_returns_the_same_active_exchange() {
    let mut sequence = SequenceCounter::new(SequenceNumber::new(7).unwrap());
    let (active, attempt) = runtime().begin_attempt(&mut sequence);
    let StaAuthenticationEvent::Irrelevant(active) = active.observe_management_frame(&[0; 30])
    else {
        panic!("irrelevant frame")
    };
    let StaAuthenticationEvent::Authenticated { attempt: completed } =
        active.observe_management_frame(&authentication_response(0))
    else {
        panic!("selected peer authenticates")
    };
    assert_eq!(completed, attempt.ordinal);
}

#[test]
fn peer_disconnect_returns_a_retry_owner_but_rejection_consumes_the_epoch() {
    let mut sequence = SequenceCounter::new(SequenceNumber::new(0).unwrap());
    let (active, _) = runtime().begin_attempt(&mut sequence);
    let StaAuthenticationEvent::Retry {
        runtime,
        attempt,
        failure,
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
    let (active, attempt) = runtime.begin_attempt(&mut sequence);
    assert_eq!(attempt.ordinal, 2);
    assert!(matches!(
        active.observe_management_frame(&authentication_response(17)),
        StaAuthenticationEvent::Failed {
            attempts: 2,
            failure: StaAuthenticationFailure::Rejected { status_code: 17 },
        }
    ));
}
