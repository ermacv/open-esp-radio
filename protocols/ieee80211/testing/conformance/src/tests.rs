use super::*;
use alloc::{vec, vec::Vec};

type TestBarrierTrace =
    BarrierTrace<Vec<BarrierEvent<(), ()>>, Vec<u8>, Vec<Vec<u8>>, BarrierEvent<(), ()>>;

fn retry_trace() -> TestBarrierTrace {
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
        retry_terminal: None,
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

fn poisoned_trace() -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
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
                    end: End::Ended,
                }
            })
            .collect(),
        contexts_dropped: 2,
        before_teardown_dropped: 2,
        publications: 2,
        terminal_evidence: vec![],
        publication_events: vec![],
    }
}

#[test]
fn poison_verification_rejects_destruction_of_an_unreturned_owner() {
    assert_eq!(verify(Scenario::Poison, &poisoned_trace()), Ok(()));
    for dropped in [0, 1, 3, 4] {
        let mut trace = poisoned_trace();
        trace.contexts_dropped = dropped;
        assert_eq!(verify(Scenario::Poison, &trace), Err(Violation::Lifetime));
    }
}

fn closed_trace() -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
    let terminal: oer_ieee80211_contracts_port::SingleOutcome<(), ()> =
        TxOutcome::Ended(SingleEnd::AckTimeout(oer_ieee80211_contracts_port::Air {
            transmissions: core::num::NonZeroU16::new(2).expect("two transmissions"),
            rate: oer_ieee80211_contracts_port::PhyRate::Legacy(
                oer_ieee80211_wire::phy::LegacyRate::Ofdm6M,
            ),
        }));
    let air = ReturnEvidence::terminal(true, &terminal).expect("terminal evidence");
    let withdrawn = ReturnEvidence::terminal(
        true,
        &oer_ieee80211_contracts_port::SingleOutcome::<(), ()>::Withdrawn(
            WithdrawCause::GateClosed,
        ),
    )
    .expect("withdrawal evidence");
    Trace {
        returned: [2, 3, 0, 1]
            .into_iter()
            .map(|id| {
                let operation = OPERATIONS[id];
                let resources = Resources {
                    ticket: operation.id,
                    buffer: operation.id,
                    class: operation.class,
                    body: id,
                };
                Returned {
                    operation,
                    expected: resources,
                    actual: resources,
                    end: if id < 2 { End::Withdrawn } else { End::Ended },
                }
            })
            .collect(),
        contexts_dropped: 4,
        before_teardown_dropped: 4,
        publications: 4,
        terminal_evidence: vec![air, air, withdrawn, withdrawn],
        publication_events: vec![
            PublicationEvent::Tx(2),
            PublicationEvent::Tx(2),
            PublicationEvent::Tx(3),
            PublicationEvent::Tx(3),
            PublicationEvent::GateClosed,
        ],
    }
}

#[test]
fn close_gate_rejects_a_ticket_count_different_from_its_terminal() {
    let mut trace = closed_trace();
    assert_eq!(verify(Scenario::CloseGate, &trace), Ok(()));
    let terminal: oer_ieee80211_contracts_port::SingleOutcome<(), ()> =
        TxOutcome::Ended(SingleEnd::AckTimeout(oer_ieee80211_contracts_port::Air {
            transmissions: core::num::NonZeroU16::MIN,
            rate: oer_ieee80211_contracts_port::PhyRate::Legacy(
                oer_ieee80211_wire::phy::LegacyRate::Ofdm6M,
            ),
        }));
    trace.terminal_evidence[0] =
        ReturnEvidence::terminal(true, &terminal).expect("terminal evidence");
    assert_eq!(
        verify(Scenario::CloseGate, &trace),
        Err(Violation::Publication)
    );
}

#[test]
fn close_gate_rejects_publication_of_a_fresh_withdrawn_ticket() {
    let mut trace = closed_trace();
    assert_eq!(verify(Scenario::CloseGate, &trace), Ok(()));
    trace.publication_events.insert(4, PublicationEvent::Tx(0));
    trace.publications += 1;
    assert_eq!(
        verify(Scenario::CloseGate, &trace),
        Err(Violation::Publication)
    );
}

#[test]
fn close_gate_rejects_publication_after_confirmation_with_all_counts_correct() {
    let mut trace = closed_trace();
    assert_eq!(verify(Scenario::CloseGate, &trace), Ok(()));
    trace.publication_events.swap(3, 4);
    assert_eq!(verify(Scenario::CloseGate, &trace), Err(Violation::Order));
}

#[test]
fn publication_comparison_accepts_different_interleaving_between_access_categories() {
    use PublicationEvent::{GateClosed, Tx};
    let left = [Tx(0), Tx(2), Tx(1), Tx(3), GateClosed];
    let right = [Tx(2), Tx(0), Tx(3), Tx(1), GateClosed];
    assert!(same_publications_by_ac(
        left.into_iter(),
        right.into_iter(),
        |ticket| {
            OPERATIONS
                .get(usize::from(ticket))
                .map(|operation| operation.category)
        }
    ));
}

#[test]
fn publication_comparison_rejects_reordering_within_one_access_category() {
    use PublicationEvent::{GateClosed, Tx};
    let left = [Tx(0), Tx(2), Tx(1), Tx(3), GateClosed];
    // The only changed per-AC projection is BE: [0,1] becomes [1,0].
    let right = [Tx(2), Tx(1), Tx(3), Tx(0), GateClosed];
    assert!(!same_publications_by_ac(
        left.into_iter(),
        right.into_iter(),
        |ticket| {
            OPERATIONS
                .get(usize::from(ticket))
                .map(|operation| operation.category)
        }
    ));
}

#[test]
fn edca_verifier_rejects_tail_parameters_wrong_draws_and_queue_reordering() {
    use super::edca::{self, Case, Draw, Scenario};
    use oer_ieee80211_contracts_port::VifRole::{AccessPoint, Station};
    let case = Case {
        scenario: Scenario::AlternatingVifs,
        category: WmmAccessCategory::BestEffort,
    };
    let trace = || {
        let station = edca::parameters(Station, false);
        let ap = edca::parameters(AccessPoint, false);
        let draw = |role, table: oer_ieee80211_contracts_port::EdcaParameterSet, slots| Draw {
            role,
            category: case.category,
            parameters: table.access_category(case.category),
            window_slots: slots,
            backoff_slots: slots,
        };
        edca::Trace {
            draws: [
                draw(Station, station, 15),
                draw(AccessPoint, ap, 3),
                draw(Station, station, 15),
            ],
            installed: [station, ap],
            admitted: [1, 2, 3],
            completed: [1, 2, 3],
            held_draws: None,
        }
    };
    assert_eq!(edca::verify(case, trace()), Ok(()));
    let mut wrong = trace();
    wrong.draws[0].parameters = wrong.draws[1].parameters;
    assert_eq!(
        edca::verify(case, wrong),
        Err(edca::Violation::VifParameters)
    );
    let mut wrong = trace();
    wrong.draws[1].window_slots = 15;
    assert_eq!(edca::verify(case, wrong), Err(edca::Violation::Contention));
    let mut wrong = trace();
    wrong.draws[2].backoff_slots = 0;
    assert_eq!(edca::verify(case, wrong), Err(edca::Violation::Contention));
    let mut wrong = trace();
    wrong.completed.swap(0, 2);
    assert_eq!(edca::verify(case, wrong), Err(edca::Violation::Fifo));
    let mut wrong = trace();
    wrong.installed[1] = wrong.installed[0];
    assert_eq!(
        edca::verify(case, wrong),
        Err(edca::Violation::VifParameters)
    );
}
