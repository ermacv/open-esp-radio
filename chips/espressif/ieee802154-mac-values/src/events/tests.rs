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
