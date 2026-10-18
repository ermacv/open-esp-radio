use oer_esp32s31_ieee80211_mac::irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};

use super::*;

#[test]
fn early_deadline_still_observes_completion_without_aborting() {
    let phase = AggregateServicePhase::Published(oer_time::Instant::from_micros(100));
    assert_eq!(
        phase
            .action(WifiTxWake::Deadline, || Ok(oer_time::Instant::from_micros(
                99
            )))
            .unwrap(),
        AggregateServiceAction::Observe(AggregateTxServiceEvent::Pending)
    );
    assert_eq!(
        phase
            .action(WifiTxWake::Deadline, || Ok(oer_time::Instant::from_micros(
                100
            )))
            .unwrap(),
        AggregateServiceAction::Observe(AggregateTxServiceEvent::ExecutorDeadline)
    );
}

#[test]
fn published_interrupts_do_not_sample_the_clock() {
    for (events, expected) in [
        (EVENT_TX_COMPLETE, AggregateTxServiceEvent::Completion),
        (EVENT_TX_TIMEOUT, AggregateTxServiceEvent::HardwareTimeout),
        (EVENT_COLLISION, AggregateTxServiceEvent::Collision),
    ] {
        assert_eq!(
            AggregateServicePhase::Published(oer_time::Instant::from_micros(100))
                .action(WifiTxWake::Interrupt { events }, || panic!(
                    "unneeded clock read"
                ))
                .unwrap(),
            AggregateServiceAction::Observe(expected)
        );
    }
}

#[test]
fn every_wake_preserves_abort_until_the_post_request_deadline() {
    let phase = AggregateServicePhase::after_abort(oer_time::Instant::from_micros(100)).unwrap();
    assert_eq!(phase.deadline(), oer_time::Instant::from_micros(116));
    for wake in [
        WifiTxWake::Deadline,
        WifiTxWake::Interrupt {
            events: EVENT_TX_TIMEOUT,
        },
        WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
        },
        WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE | EVENT_COLLISION,
        },
        WifiTxWake::Interrupt { events: 0 },
    ] {
        for now in 100..116 {
            assert_eq!(
                phase
                    .action(wake, || Ok(oer_time::Instant::from_micros(now)))
                    .unwrap(),
                AggregateServiceAction::Wait
            );
        }
        assert_eq!(
            phase
                .action(wake, || Ok(oer_time::Instant::from_micros(116)))
                .unwrap(),
            AggregateServiceAction::FinishAbort
        );
    }
}

#[test]
fn conflicting_published_interrupts_fail_closed() {
    assert!(matches!(
        AggregateServicePhase::Published(oer_time::Instant::from_micros(100)).action(
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT
            },
            || panic!("conflict must not read time")
        ),
        Err(AccessPointDatapathError::Aggregate(
            ApAmpduError::ConflictingInterruptEvents(_)
        ))
    ));
}

#[test]
fn overflow_cannot_turn_into_a_wrapped_or_successful_abort() {
    assert!(
        AggregateServicePhase::after_abort(oer_time::Instant::from_micros(u64::MAX - 15)).is_err()
    );
    let phase =
        AggregateServicePhase::after_abort(oer_time::Instant::from_micros(u64::MAX - 16)).unwrap();
    assert_eq!(phase.deadline(), oer_time::Instant::from_micros(u64::MAX));
    assert_eq!(
        phase
            .action(WifiTxWake::Deadline, || Ok(oer_time::Instant::from_micros(
                u64::MAX - 1
            )))
            .unwrap(),
        AggregateServiceAction::Wait
    );
    assert!(
        AggregateServicePhase::ResetRequired
            .action(
                WifiTxWake::Interrupt {
                    events: EVENT_TX_COMPLETE
                },
                || panic!("fault is terminal")
            )
            .is_err()
    );
}
