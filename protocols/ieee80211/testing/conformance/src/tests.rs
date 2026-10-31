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

/// Evidence of one acknowledged publication.
fn acknowledged(transmissions: u16) -> ReturnEvidence {
    let terminal: oer_ieee80211_contracts_port::SingleOutcome<(), ()> =
        TxOutcome::Ended(SingleEnd::AckTimeout(oer_ieee80211_contracts_port::Air {
            transmissions: core::num::NonZeroU16::new(transmissions).expect("a transmission"),
            rate: oer_ieee80211_contracts_port::PhyRate::Legacy(
                oer_ieee80211_wire::phy::LegacyRate::Ofdm6M,
            ),
        }));
    ReturnEvidence::terminal(true, &terminal).expect("terminal evidence")
}

fn withdrawn() -> ReturnEvidence {
    ReturnEvidence::terminal(
        true,
        &oer_ieee80211_contracts_port::SingleOutcome::<(), ()>::Withdrawn(
            WithdrawCause::GateClosed,
        ),
    )
    .expect("withdrawal evidence")
}

/// Rows of operations `ids` in return order, each with its `end`.
fn rows(ids: &[(usize, End)]) -> Vec<Returned> {
    ids.iter()
        .map(|&(id, end)| {
            let operation = Classes::Two.operations()[id];
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
                end,
            }
        })
        .collect()
}

fn poisoned_trace() -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
    use PublicationEvent::{Requested, Returned as Back, Tx};
    Trace {
        returned: rows(&[(2, End::Ended), (3, End::Ended)]),
        contexts_dropped: 2,
        before_teardown_dropped: 2,
        publications: 2,
        terminal_evidence: vec![acknowledged(1), acknowledged(1)],
        publication_events: vec![Tx(2), Tx(3), Back(2), Back(3), Requested],
        tickets: [0, 1, 2, 3],
    }
}

#[test]
fn poison_verification_rejects_destruction_of_an_unreturned_owner() {
    assert_eq!(
        verify(
            Scenario::Poison,
            Classes::Two,
            Held::Unpublished,
            &poisoned_trace()
        ),
        Ok(())
    );
    for dropped in [0, 1, 3, 4] {
        let mut trace = poisoned_trace();
        trace.contexts_dropped = dropped;
        assert_eq!(
            verify(Scenario::Poison, Classes::Two, Held::Unpublished, &trace),
            Err(Violation::Lifetime)
        );
    }
}

/// VO and VI end after two transmissions; then the gate closes over the
/// unpublished BestEffort pair.
fn closed_trace() -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
    use PublicationEvent::{GateClosed, Requested, Returned as Back, Tx};
    Trace {
        returned: rows(&[
            (2, End::Ended),
            (3, End::Ended),
            (0, End::Withdrawn),
            (1, End::Withdrawn),
        ]),
        contexts_dropped: 4,
        before_teardown_dropped: 4,
        publications: 4,
        terminal_evidence: vec![acknowledged(2), acknowledged(2), withdrawn(), withdrawn()],
        publication_events: vec![
            Tx(2),
            Tx(2),
            Tx(3),
            Tx(3),
            Back(2),
            Back(3),
            Requested,
            Back(0),
            Back(1),
            GateClosed,
        ],
        tickets: [0, 1, 2, 3],
    }
}

#[test]
fn close_gate_rejects_a_ticket_count_different_from_its_terminal() {
    let mut trace = closed_trace();
    assert_eq!(
        verify(Scenario::CloseGate, Classes::Two, Held::Unpublished, &trace),
        Ok(())
    );
    trace.terminal_evidence[0] = acknowledged(1);
    assert_eq!(
        verify(Scenario::CloseGate, Classes::Two, Held::Unpublished, &trace),
        Err(Violation::Publication)
    );
}

#[test]
fn close_gate_rejects_publication_of_a_fresh_withdrawn_ticket() {
    let mut trace = closed_trace();
    trace.publication_events.insert(9, PublicationEvent::Tx(0));
    trace.publications += 1;
    assert_eq!(
        verify(Scenario::CloseGate, Classes::Two, Held::Unpublished, &trace),
        Err(Violation::Publication)
    );
}

#[test]
fn close_gate_rejects_publication_after_confirmation_with_all_counts_correct() {
    let mut trace = closed_trace();
    let late = trace.publication_events.remove(3);
    trace.publication_events.push(late);
    assert_eq!(
        verify(Scenario::CloseGate, Classes::Two, Held::Unpublished, &trace),
        Err(Violation::Order)
    );
}

/// The BestEffort head is published and held before the gate closes; it
/// then ends with its own terminal and the tail is withdrawn.
fn closed_head_trace() -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
    use PublicationEvent::{GateClosed, Requested, Returned as Back, Tx};
    Trace {
        returned: rows(&[
            (2, End::Ended),
            (3, End::Ended),
            (0, End::Ended),
            (1, End::Withdrawn),
        ]),
        contexts_dropped: 4,
        before_teardown_dropped: 4,
        publications: 3,
        terminal_evidence: vec![
            acknowledged(1),
            acknowledged(1),
            acknowledged(1),
            withdrawn(),
        ],
        publication_events: vec![
            Tx(0),
            Tx(2),
            Tx(3),
            Back(2),
            Back(3),
            Requested,
            Back(0),
            Back(1),
            GateClosed,
        ],
        tickets: [0, 1, 2, 3],
    }
}

#[test]
fn a_published_head_ends_with_its_terminal_and_the_tail_is_withdrawn() {
    let trace = closed_head_trace();
    assert_eq!(
        verify(
            Scenario::CloseGate,
            Classes::Two,
            Held::PublishedHead,
            &trace
        ),
        Ok(())
    );
    // The same observations do not satisfy the other mode.
    assert_ne!(
        verify(Scenario::CloseGate, Classes::Two, Held::Unpublished, &trace),
        Ok(())
    );
}

#[test]
fn the_published_head_mode_requires_its_precondition_at_the_request() {
    use PublicationEvent::{Requested, Tx};
    // The head published only after the request.
    let mut late = closed_head_trace();
    late.publication_events.remove(0);
    let at = late
        .publication_events
        .iter()
        .position(|event| *event == Requested)
        .expect("request");
    late.publication_events.insert(at + 1, Tx(0));
    assert_eq!(
        verify(
            Scenario::CloseGate,
            Classes::Two,
            Held::PublishedHead,
            &late
        ),
        Err(Violation::Publication)
    );
    // The tail also published before the request.
    let mut tail = closed_head_trace();
    tail.publication_events.insert(1, Tx(1));
    tail.publications += 1;
    assert_eq!(
        verify(
            Scenario::CloseGate,
            Classes::Two,
            Held::PublishedHead,
            &tail
        ),
        Err(Violation::Publication)
    );
}

#[test]
fn the_gate_confirms_only_after_both_best_effort_returns() {
    let mut trace = closed_head_trace();
    let gate = trace.publication_events.pop().expect("gate");
    trace.publication_events.insert(6, gate);
    assert_eq!(
        verify(
            Scenario::CloseGate,
            Classes::Two,
            Held::PublishedHead,
            &trace
        ),
        Err(Violation::Order)
    );
}

#[test]
fn poison_keeps_a_published_head_and_its_tail_quarantined() {
    use PublicationEvent::{Requested, Returned as Back, Tx};
    let mut trace = poisoned_trace();
    trace.publication_events = vec![Tx(0), Tx(2), Tx(3), Back(2), Back(3), Requested];
    trace.publications = 3;
    assert_eq!(
        verify(Scenario::Poison, Classes::Two, Held::PublishedHead, &trace),
        Ok(())
    );
    // A return of the published head is not a quarantine.
    trace.publication_events.push(Back(0));
    assert_eq!(
        verify(Scenario::Poison, Classes::Two, Held::PublishedHead, &trace),
        Err(Violation::Missing)
    );
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
            Classes::Two
                .operations()
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
            Classes::Two
                .operations()
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

#[test]
fn a_one_class_backend_runs_the_same_order_and_categories_in_its_one_class() {
    let two = Classes::Two.operations();
    let one = Classes::One.operations();
    for (two, one) in two.iter().zip(one) {
        assert_eq!((two.id, two.category), (one.id, one.category));
        assert_eq!(one.class, Class::Small);
    }
    assert!(two.iter().any(|operation| operation.class == Class::Large));
}

#[test]
fn observed_order_places_each_marker_before_later_publications() {
    use PublicationEvent::{GateClosed, Requested, Returned as Back, Tx};
    let order: Vec<_> = observed_order(
        [0, 2, 3],
        [
            (3, Back(2)),
            (3, Back(3)),
            (3, Requested),
            (3, Back(0)),
            (3, GateClosed),
        ],
    )
    .collect();
    assert_eq!(
        order,
        [
            Tx(0),
            Tx(2),
            Tx(3),
            Back(2),
            Back(3),
            Requested,
            Back(0),
            GateClosed
        ]
    );
    let early: Vec<_> = observed_order([5], [(0, Requested)]).collect();
    assert_eq!(early, [Requested, Tx(5)]);
}
