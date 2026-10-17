use super::*;
use alloc::vec;

fn retry_trace() -> BarrierTrace<(), ()> {
    let mut original = vec![0x5a; 40];
    original[0] = 0x08;
    original[1] = 0x41;
    let mut retry = original.clone();
    retry[1] |= 0x08;
    BarrierTrace {
        tickets: [1, 2],
        before_request: HeldSnapshot {
            publications: 1,
            events: 0,
        },
        before_result: HeldSnapshot {
            publications: 1,
            events: 0,
        },
        just_before_timeout: None,
        publications_at_barrier: 1,
        events: vec![
            BarrierEvent::Tx {
                ticket: 1,
                state: MpduState::Sent,
                outcome: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
            },
            BarrierEvent::Tx {
                ticket: 2,
                state: MpduState::Fresh,
                outcome: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
            },
            BarrierEvent::GateClosed,
        ],
        first_frame: original,
        retried_frames: vec![retry],
    }
}

#[test]
fn changed_retry_bytes_are_distinct_from_returning_the_wrong_owner() {
    assert_eq!(
        verify_barrier(BarrierCase::GateRetry, retry_trace()),
        Ok(())
    );
    // Header, SN, PN and payload changes all belong to the MPDU contents
    // diagnostic. Ticket identity is unchanged in every case.
    for index in 0..40 {
        let mut trace = retry_trace();
        trace.retried_frames[0][index] ^= 1;
        let error = verify_barrier(BarrierCase::GateRetry, trace).unwrap_err();
        assert_ne!(error, Violation::Owners, "changed octet {index}");
        assert_eq!(error, Violation::MpduChanged);
    }
}

#[test]
fn changed_ticket_identity_is_an_owner_violation() {
    let mut trace = retry_trace();
    trace.tickets[0] = 3;
    assert_eq!(
        verify_barrier(BarrierCase::GateRetry, trace),
        Err(Violation::Owners)
    );
}

fn poisoned_trace() -> Trace {
    Trace {
        returned: OPERATIONS
            .into_iter()
            .filter(|operation| operation.category != WmmAccessCategory::BestEffort)
            .map(|operation| {
                let resources = Resources {
                    ticket: operation.id,
                    buffer: operation.id,
                    class: operation.class,
                    body: usize::from(operation.id),
                };
                Returned {
                    operation,
                    expected: resources,
                    actual: resources,
                    end: End::Completed,
                }
            })
            .collect(),
        contexts_dropped: 2,
        before_teardown_dropped: 2,
        publications: 2,
    }
}

#[test]
fn poison_verification_rejects_destruction_of_an_unreturned_owner() {
    assert_eq!(verify(Scenario::Poison, poisoned_trace()), Ok(()));
    for dropped in [0, 1, 3, 4] {
        let mut trace = poisoned_trace();
        trace.contexts_dropped = dropped;
        assert_eq!(verify(Scenario::Poison, trace), Err(Violation::Lifetime));
    }
}
