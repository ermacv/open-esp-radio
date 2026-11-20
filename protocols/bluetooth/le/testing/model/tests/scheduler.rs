//! The model's scheduler, rounds, facts, anomalies, starts and counts.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]

mod harness;

use harness::{Model, advertising_event, at, cx, installed, nonconnectable, running};
use oer_protocols_bluetooth_le_contracts_port::{
    AdvEvent, Advertiser, EventEnd, EventPlace, Facts, LePort, NotExecuted, Rest, RoleRights,
    Signal, StartFailure, StartPoll,
};
use oer_protocols_bluetooth_le_testing_model::{Anomaly, EventId, Role, StartScript};

type Port<'id> = Model<'id, 1, 0, 0, 2, 0, 0, 0>;

const SET: EventId = EventId {
    role: Role::Advertising,
    instance: 0,
    slot: 0,
};

/// The set's one event's end and its place, from one round.
fn ended<'id>(port: &mut Port<'id>) -> (EventEnd, AdvEvent<'id>, Rest) {
    let mut round = port.round();
    let ended = round.ended().expect("an end");
    let end = ended.end();
    let EventPlace::Advertising(home) = ended.place() else {
        panic!("the set's place");
    };
    (end, home, round.close())
}

/// Open the set on a running radio, run `body`, close it.
fn with_set(
    body: impl for<'id> FnOnce(&mut Port<'id>, &Advertiser<'id>, AdvEvent<'id>) -> AdvEvent<'id>,
) {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        let (stopped, place) = running(port, stopped, &mut right, |port| {
            let (role, home) = port.open_advertising(place, &nonconnectable());
            let home = body(port, &role, home);
            port.close_advertising(role, home)
        });
        let roles = RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn an_event_published_later_than_its_start_less_the_guard_ends_late() {
    with_set(|port, _, home| {
        port.advance(at(10_000));
        // Its reservation starts 100 us before 10_120: 20 us after now.
        let _published = port.publish_advertising(home, &advertising_event(at(10_120), at(0)));
        let (end, home, _) = ended(port);
        assert_eq!(end, EventEnd::NotExecuted(NotExecuted::Late));
        assert_eq!(port.listed(), 0, "nothing was listed");
        home
    });
}

#[test]
fn a_displaced_event_ends_displaced_at_its_start() {
    with_set(|port, _, home| {
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        port.displace(SET);
        port.advance(at(1_999));
        assert!(port.round().ended().is_none(), "not before its start");
        port.advance(at(2_000));
        let (end, home, _) = ended(port);
        assert_eq!(end, EventEnd::NotExecuted(NotExecuted::Displaced));
        home
    });
}

#[test]
fn a_withdraw_ends_a_listed_event_withdrawn() {
    with_set(|port, role, home| {
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        port.withdraw(oer_protocols_bluetooth_le_contracts_port::RoleRef::Advertiser(role));
        let (end, home, rest) = ended(port);
        assert_eq!(end, EventEnd::NotExecuted(NotExecuted::Withdrawn));
        assert_eq!(rest, Rest::Quiet);
        home
    });
}

#[test]
fn a_rest_names_what_comes_next() {
    with_set(|port, _, home| {
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        let round = port.round();
        assert_eq!(round.close(), Rest::At(at(2_000)), "its start");
        port.advance(at(3_000));
        let round = port.round();
        assert_eq!(round.close(), Rest::At(at(6_500)), "its end");
        port.advance(at(6_500));
        let (_, home, rest) = ended(port);
        assert_eq!(rest, Rest::Quiet);
        home
    });
}

#[test]
fn an_abandoned_round_leaves_its_ends_for_the_next() {
    with_set(|port, _, home| {
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        port.advance(at(7_000));
        {
            let round = port.round();
            drop(round);
        }
        let (end, home, _) = ended(port);
        assert!(matches!(end, EventEnd::Executed(_)));
        home
    });
}

#[test]
fn the_facts_say_what_finished_and_when_the_alarm_passed() {
    with_set(|port, _, home| {
        assert_eq!(port.poll_facts(&mut cx()), Facts::default());
        port.alarm(Some(at(3_000)));
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        port.advance(at(2_999));
        assert_eq!(port.poll_facts(&mut cx()).alarm, Signal::Quiet);
        port.advance(at(7_000));
        let facts = port.poll_facts(&mut cx());
        assert_eq!(
            (facts.finished, facts.alarm),
            (Signal::Raised, Signal::Raised)
        );
        port.alarm(None);
        let (_, home, _) = ended(port);
        assert_eq!(port.poll_facts(&mut cx()), Facts::default());
        home
    });
}

#[test]
fn after_an_anomaly_a_round_is_empty_until_the_stop() {
    for anomaly in [
        Anomaly::ExceptionLane,
        Anomaly::Sentinel,
        Anomaly::UnexecutedLeft,
        Anomaly::LockAnswer,
        Anomaly::Bound,
    ] {
        installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
            let RoleRights {
                advertising: [place],
                scanners,
                connections,
                tests,
                accept_list,
            } = roles;
            let (stopped, role) = running(port, stopped, &mut right, |port| {
                let (role, home) = port.open_advertising(place, &nonconnectable());
                let _published =
                    port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
                port.raise(anomaly);
                assert_eq!(port.poll_facts(&mut cx()).needs_stop, Signal::Raised);
                port.advance(at(10_000));
                let mut round = port.round();
                assert!(round.ended().is_none(), "{anomaly:?}");
                assert_eq!(round.close(), Rest::Quiet);
                role
            });
            assert_eq!(port.poll_facts(&mut cx()).needs_stop, Signal::Quiet);
            let (end, home, _) = ended(port);
            assert_eq!(
                end,
                EventEnd::NotExecuted(NotExecuted::Withdrawn),
                "{anomaly:?}"
            );
            let place = port.close_advertising(role, home);
            let roles = RoleRights {
                advertising: [place],
                scanners,
                connections,
                tests,
                accept_list,
            };
            let retired = port.retire(stopped, roles);
            port.release(retired, right);
        });
    }
}

#[test]
fn a_run_after_an_anomaly_is_as_a_run_after_an_ordinary_stop() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        let mut place = place;
        let mut stopped = stopped;
        for raise in [false, true, false] {
            let begin = port.now().as_micros() + 1_000_000;
            port.advance(at(begin));
            let (next, role) = running(port, stopped, &mut right, |port| {
                let (role, home) = port.open_advertising(place, &nonconnectable());
                let _published = port
                    .publish_advertising(home, &advertising_event(at(begin + 2_000), port.now()));
                if raise {
                    port.raise(Anomaly::Bound);
                }
                role
            });
            stopped = next;
            let (_, home, _) = ended(port);
            place = port.close_advertising(role, home);
        }
        let runs = port.runs();
        assert_eq!(
            runs[0], runs[2],
            "the run after the anomaly's is as the first"
        );
        let roles = RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_failed_start_answers_stopped_with_its_cause_and_the_next_start_is_cold() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        for failure in [
            StartFailure::Phy,
            StartFailure::SharedOwner,
            StartFailure::Bound,
        ] {
            port.script_start(StartScript::Fail(failure));
        }
        let mut stopped = stopped;
        for failure in [
            StartFailure::Phy,
            StartFailure::SharedOwner,
            StartFailure::Bound,
        ] {
            let starting = port.start(stopped);
            let StartPoll::Stopped(next, cause) = port.poll_start(starting, &mut cx()) else {
                panic!("the scripted start fails");
            };
            assert_eq!(cause, failure);
            stopped = next;
        }
        let (stopped, ()) = running(port, stopped, &mut right, |_| ());
        assert_eq!(port.runs().len(), 4);
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn the_counts_name_every_call() {
    with_set(|port, _, home| {
        let before = port.counts();
        let _published = port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
        port.advance(at(7_000));
        let (_, home, _) = ended(port);
        let after = port.counts();
        assert_eq!(after.publications - before.publications, 1);
        assert_eq!(after.rounds - before.rounds, 1);
        // The publication, and the round's open, its one end and its close.
        assert_eq!(after.calls - before.calls, 4);
        home
    });
}

#[test]
fn an_end_from_before_an_anomaly_is_given_only_after_the_stop() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        let (stopped, role) = running(port, stopped, &mut right, |port| {
            let (role, home) = port.open_advertising(place, &nonconnectable());
            let _published =
                port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
            port.advance(at(7_000));
            port.raise(Anomaly::Sentinel);
            let mut round = port.round();
            assert!(round.ended().is_none(), "a round is empty until the stop");
            assert_eq!(round.close(), Rest::Quiet);
            assert_eq!(port.poll_facts(&mut cx()).finished, Signal::Quiet);
            role
        });
        let (end, home, _) = ended(port);
        assert!(
            matches!(end, EventEnd::Executed(_)),
            "it ended before the anomaly"
        );
        let place = port.close_advertising(role, home);
        let roles = RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}
