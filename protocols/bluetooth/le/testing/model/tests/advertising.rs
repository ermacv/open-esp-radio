//! The thin consumer: one advertising set through the model, from the
//! install to the release, and a second run's record equal to the first's.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]

mod harness;

use harness::{Model, advertising_event, at, cx, installed, nonconnectable};
use oer_protocols_bluetooth_le_contracts_port::{
    Cut, EventEnd, EventPlace, Executed, LePort, NotExecuted, Rest, RoleRights, StartPoll,
    StopFrom, StopPoll, StopRight, Stopped,
};
use oer_protocols_bluetooth_le_testing_model::{EventId, Programmed, Role};

type Port<'id> = Model<'id, 1, 0, 0, 2, 0, 0, 0>;
type Roles<'id> = RoleRights<'id, 1, 0, 0, 0>;

/// Where a run's stop is taken.
#[derive(Clone, Copy, Debug)]
enum StopAt {
    /// After the event ended and its set closed.
    Ended,
    /// While the event is listed, not begun.
    Listed,
    /// While the event runs.
    Running,
}

/// One run from `begin`: start, open the set, publish an event 2 ms ahead,
/// move the clock as `stop_at` says, the event's end in a round, the set's
/// closing and the stop, in the order `stop_at` names. Answers the end.
fn run<'id>(
    port: &mut Port<'id>,
    stopped: Stopped<'id>,
    right: &mut StopRight<'id>,
    roles: Roles<'id>,
    stop_at: StopAt,
    begin: u64,
) -> (Stopped<'id>, Roles<'id>, EventEnd) {
    let RoleRights {
        advertising: [place],
        scanners,
        connections,
        tests,
        accept_list,
    } = roles;
    port.advance(at(begin));
    let starting = port.start(stopped);
    let StartPoll::Running(running) = port.poll_start(starting, &mut cx()) else {
        panic!("the model's start confirms");
    };
    let (role, home) = port.open_advertising(place, &nonconnectable());
    let anchor = begin + 2_000;
    let _published = port.publish_advertising(home, &advertising_event(at(anchor), port.now()));
    let stop = |port: &mut Port<'id>, right: &mut StopRight<'id>, running| {
        let stopping = port.stop(right, StopFrom::Running(running));
        let StopPoll::Stopped(stopped) = port.poll_stop(stopping, &mut cx()) else {
            panic!("the model's stop ends at once");
        };
        stopped
    };
    let radio = match stop_at {
        StopAt::Ended => {
            port.advance(at(anchor + 4_500));
            Radio::Running(running)
        }
        StopAt::Listed | StopAt::Running => {
            if let StopAt::Running = stop_at {
                port.advance(at(anchor + 1_000));
            }
            let mut round = port.round();
            assert!(round.ended().is_none(), "nothing ended before the stop");
            assert_ne!(round.close(), Rest::Quiet, "the event is out");
            Radio::Stopped(stop(port, right, running))
        }
    };
    let mut round = port.round();
    let ended = round.ended().expect("the event's end");
    let end = ended.end();
    let EventPlace::Advertising(home) = ended.place() else {
        panic!("an advertising event's place");
    };
    assert!(round.ended().is_none(), "one end");
    assert_eq!(round.close(), Rest::Quiet);
    let place = port.close_advertising(role, home);
    let stopped = match radio {
        Radio::Running(running) => stop(port, right, running),
        Radio::Stopped(stopped) => stopped,
    };
    let roles = RoleRights {
        advertising: [place],
        scanners,
        connections,
        tests,
        accept_list,
    };
    (stopped, roles, end)
}

/// The radio's right after the event was taken care of.
enum Radio<'id> {
    Running(oer_protocols_bluetooth_le_contracts_port::Running<'id>),
    Stopped(Stopped<'id>),
}

#[test]
fn an_advertising_event_ends_executed_and_its_place_comes_home() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let (stopped, roles, end) = run(port, stopped, &mut right, roles, StopAt::Ended, 0);
        assert_eq!(
            end,
            EventEnd::Executed(Executed {
                anchor: Some(at(2_000)),
                exchanged: 3,
                cut: Cut::Whole,
                peer: None,
            })
        );
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_stop_ends_a_listed_event_withdrawn() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let (stopped, roles, end) = run(port, stopped, &mut right, roles, StopAt::Listed, 0);
        assert_eq!(end, EventEnd::NotExecuted(NotExecuted::Withdrawn));
        assert_eq!(port.listed(), 0);
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_stop_ends_a_running_event_indeterminate() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let (stopped, roles, end) = run(port, stopped, &mut right, roles, StopAt::Running, 0);
        assert_eq!(end, EventEnd::Indeterminate);
        assert_eq!(port.listed(), 0);
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_run_records_what_it_programmed_and_answered() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
        let (stopped, roles, _) = run(port, stopped, &mut right, roles, StopAt::Ended, 0);
        let event = EventId {
            role: Role::Advertising,
            instance: 0,
            slot: 0,
        };
        assert_eq!(
            port.runs(),
            [std::vec![
                Programmed::Start,
                Programmed::Started(None),
                Programmed::Opened(Role::Advertising, 0),
                Programmed::Published(
                    event,
                    oer_protocols_bluetooth_le_contracts_port::RadioDuration::from_micros(2_000)
                ),
                Programmed::Ended(
                    event,
                    oer_protocols_bluetooth_le_testing_model::Ending::Executed {
                        anchor: Some(
                            oer_protocols_bluetooth_le_contracts_port::RadioDuration::from_micros(
                                2_000
                            )
                        ),
                        exchanged: 3,
                        cut: Cut::Whole,
                        peer: None,
                    }
                ),
                Programmed::Closed(Role::Advertising, 0),
                Programmed::Stop,
                Programmed::Reset,
            ]]
        );
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_second_run_is_as_the_first_after_each_kind_of_stop() {
    for stop_at in [StopAt::Ended, StopAt::Listed, StopAt::Running] {
        installed::<1, 0, 0, 2, 0, 0, 0>(|port, stopped, mut right, roles| {
            let (stopped, roles, first) = run(port, stopped, &mut right, roles, stop_at, 0);
            let (stopped, roles, second) =
                run(port, stopped, &mut right, roles, stop_at, 1_000_000);
            assert_eq!(first, second.clone_anchor_from(1_000_000), "{stop_at:?}");
            let runs = port.runs();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0], runs[1], "{stop_at:?}");
            let retired = port.retire(stopped, roles);
            port.release(retired, right);
        });
    }
}

#[test]
fn a_run_after_every_kind_of_stop_is_as_a_fresh_one() {
    installed::<1, 0, 0, 2, 0, 0, 0>(|port, mut stopped, mut right, mut roles| {
        let order = [
            StopAt::Ended,
            StopAt::Running,
            StopAt::Ended,
            StopAt::Listed,
            StopAt::Ended,
            StopAt::Running,
            StopAt::Listed,
            StopAt::Ended,
        ];
        for (number, stop_at) in order.into_iter().enumerate() {
            let begin = 1_000_000 * u64::try_from(number).unwrap();
            let (next, kept, _) = run(port, stopped, &mut right, roles, stop_at, begin);
            stopped = next;
            roles = kept;
        }
        let runs = port.runs();
        for (number, stop_at) in order.into_iter().enumerate() {
            let fresh = order.iter().position(|kind| kind.same(stop_at)).unwrap();
            assert_eq!(runs[number], runs[fresh], "run {number}: {stop_at:?}");
        }
        assert_eq!(port.free_nodes(), 0, "this geometry has no nodes");
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

impl StopAt {
    fn same(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Ended, Self::Ended)
                | (Self::Listed, Self::Listed)
                | (Self::Running, Self::Running)
        )
    }
}

/// An end with its anchor moved back by `micros`, to compare a later run's
/// with an earlier's.
trait Earlier {
    fn clone_anchor_from(self, micros: u64) -> Self;
}

impl Earlier for EventEnd {
    fn clone_anchor_from(self, micros: u64) -> Self {
        match self {
            Self::Executed(executed) => Self::Executed(Executed {
                anchor: executed
                    .anchor
                    .map(|anchor| at(anchor.as_micros() - micros)),
                ..executed
            }),
            other => other,
        }
    }
}
