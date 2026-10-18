use oer_hil_protocol::ieee802154::Ieee802154ObservedEventState::{
    Clear, Timer0AndTimer1, Timer0Only, Timer1Only, UnexpectedNamed,
};

use super::*;

fn evidence(outcome: Ieee802154SameBitOutcome) -> Ieee802154RouteProbeEvidence {
    let same_bit = match outcome {
        Ieee802154SameBitOutcome::Retained => {
            vec![entry(Timer0Only, Timer0Only), entry(Timer0Only, Timer0Only)]
        }
        _ => vec![entry(Timer0Only, Timer0Only)],
    };
    Ieee802154RouteProbeEvidence {
        stop: Ieee802154RouteProbeStop::Complete,
        polled_snapshot: Timer0Only,
        polled_after_acknowledgement: if outcome == Ieee802154SameBitOutcome::Retained {
            Timer0Only
        } else {
            Clear
        },
        polled_control: Timer0Only,
        polled_outcome: outcome,
        retrigger_entries: [
            entry(Timer0Only, Timer0AndTimer1),
            entry(Timer1Only, Timer1Only),
        ]
        .into_iter()
        .collect(),
        same_bit_entries: same_bit.into_iter().collect(),
        final_events: Clear,
    }
}

#[test]
fn either_same_bit_outcome_passes_when_both_phases_agree() {
    for outcome in [
        Ieee802154SameBitOutcome::Coalesced,
        Ieee802154SameBitOutcome::Retained,
    ] {
        assert_eq!(evaluate(&evidence(outcome)).unwrap(), outcome);
    }
}

#[test]
fn disagreeing_phases_or_a_missing_retrigger_fail() {
    type Change = fn(&mut Ieee802154RouteProbeEvidence);
    let cases: [(&str, Change); 7] = [
        ("stopped", |e| {
            e.stop = Ieee802154RouteProbeStop::RouteFailed
        }),
        ("no outcome", |e| {
            e.polled_outcome = Ieee802154SameBitOutcome::NotRun
        }),
        ("control silent", |e| e.polled_control = Clear),
        ("no retrigger", |e| {
            e.retrigger_entries.truncate(1);
        }),
        ("routed disagrees", |e| {
            e.same_bit_entries
                .push(entry(Timer0Only, Timer0Only))
                .unwrap();
        }),
        ("extra entry", |e| {
            e.retrigger_entries
                .push(entry(UnexpectedNamed, UnexpectedNamed))
                .unwrap();
        }),
        ("left latched", |e| e.final_events = Timer1Only),
    ];
    for (name, change) in cases {
        let mut probe = evidence(Ieee802154SameBitOutcome::Coalesced);
        change(&mut probe);
        assert!(evaluate(&probe).is_err(), "{name}");
    }
}
