// SPDX-License-Identifier: Apache-2.0
use super::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154EventObservationError, Ieee802154ObservedEventState,
};

#[test]
fn unnamed_lanes_remain_distinct_with_and_without_named_events() {
    for named in [
        Ieee802154EventMask::NONE,
        Ieee802154Event::EdDone.mask().with(Ieee802154Event::RxDone),
    ] {
        let observations = [(false, false), (true, false), (false, true), (true, true)].map(
            |(lane_7, lane_13)| {
                let observed = Ieee802154EventObservation::from_parts(named, lane_7, lane_13);
                assert_eq!(observed.events_7(), lane_7);
                assert_eq!(observed.events_13(), lane_13);
                assert_eq!(
                    observed.contains(Ieee802154Event::EdDone),
                    named.contains(Ieee802154Event::EdDone)
                );
                assert_eq!(
                    observed.contains(Ieee802154Event::RxDone),
                    named.contains(Ieee802154Event::RxDone)
                );
                assert_eq!(observed.is_clear(), named.is_empty() && !lane_7 && !lane_13);
                if lane_7 || lane_13 {
                    assert_eq!(
                        observed.classification(),
                        Err(Ieee802154EventObservationError)
                    );
                    assert_eq!(observed.state(), Ieee802154ObservedEventState::Unclassified);
                } else {
                    assert_eq!(observed, Ieee802154EventObservation::from_named(named));
                    assert_eq!(observed.classification(), Ok(named));
                    assert_eq!(observed.state(), named.state());
                }
                observed
            },
        );
        for (index, observed) in observations.iter().enumerate() {
            for other in observations.iter().skip(index + 1) {
                assert_ne!(observed, other);
            }
        }
    }
}

#[test]
fn event_multiplicity_is_exact_for_every_named_subset() {
    let events = [
        Ieee802154Event::TxDone,
        Ieee802154Event::RxDone,
        Ieee802154Event::AckTxDone,
        Ieee802154Event::AckRxDone,
        Ieee802154Event::RxAbort,
        Ieee802154Event::TxAbort,
        Ieee802154Event::EdDone,
        Ieee802154Event::Timer0Overflow,
        Ieee802154Event::Timer1Overflow,
        Ieee802154Event::ClockCountMatch,
        Ieee802154Event::TxSfdDone,
        Ieee802154Event::RxSfdDone,
    ];
    const MULTIPLE: bool = Ieee802154EventMask::NAMED.has_multiple();
    const EMPTY: bool = Ieee802154EventMask::NONE.has_multiple();
    assert_eq!((MULTIPLE, EMPTY), (true, false));
    for bits in 0u16..(1 << events.len()) {
        let mut mask = Ieee802154EventMask::NONE;
        for (index, event) in events.into_iter().enumerate() {
            if bits & (1 << index) != 0 {
                mask = mask.with(event);
            }
        }
        assert_eq!(mask.has_multiple(), bits.count_ones() > 1);
    }
}
