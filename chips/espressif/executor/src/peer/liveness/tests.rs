extern crate std;

use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake, vec::Vec};

use oer_espressif_interrupt_table::signal::host;
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Clock as _, Duration};
use oer_time_virtual::VirtualClock;

use oer_espressif_staged_runtime::panic::FaultReason;

use super::super::Obligations;
use super::{Beat, LivenessError, Overdue, Period, PeriodError, pair};

const PERIOD: Period = match Period::new(Duration::from_millis(50), Duration::from_millis(10)) {
    Ok(period) => period,
    Err(_) => panic!("a deadline within its period"),
};

fn slot() -> &'static RequestSlot<Beat, (), InPlace> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
}

fn table() -> &'static Obligations<2> {
    Box::leak(Box::new(Obligations::new()))
}

/// A task's waker, counting its wakes.
#[derive(Default)]
struct Counting(AtomicU32);

impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn waker() -> Waker {
    Waker::from(Arc::new(Counting::default()))
}

#[test]
fn a_period_needs_a_nonzero_deadline_within_it() {
    let millis = Duration::from_millis;
    assert_eq!(Period::new(millis(50), Duration::ZERO), Err(PeriodError));
    assert_eq!(Period::new(millis(50), millis(51)), Err(PeriodError));
    let period = Period::new(millis(50), millis(50)).expect("a deadline equal to its period");
    assert_eq!(
        (period.period(), period.deadline()),
        (millis(50), millis(50))
    );
}

#[test]
fn the_seat_is_claimed_once() {
    let slot = slot();
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    assert!(pair(slot, request, reply, PERIOD, table()).is_some());
    assert!(pair(slot, request, reply, PERIOD, table()).is_none());
}

#[test]
fn rounds_complete_once_per_period_while_the_responder_echoes() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (supervisor, mut responder) =
        pair(slot(), request, reply, PERIOD, table()).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let start = clock.now();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    let mut rounds = Vec::new();
    {
        let mut supervising = pin!(supervisor.supervise(&clock, |round| rounds.push(round)));
        let mut serving = pin!(responder.serve());
        for _ in 0..3 {
            assert!(supervising.as_mut().poll(&mut cx).is_pending());
            assert!(serving.as_mut().poll(&mut cx).is_pending());
            assert!(supervising.as_mut().poll(&mut cx).is_pending());
            let _ = clock.advance(PERIOD.period());
        }
    }
    let sequences: Vec<u32> = rounds.iter().map(|round| round.sequence).collect();
    assert_eq!(sequences, [1, 2, 3]);
    for (index, round) in (0_u64..).zip(&rounds) {
        // Each round starts one period after the previous one started.
        assert_eq!(
            round.posted.checked_duration_since(start),
            Some(Duration::from_micros(index * PERIOD.period().as_micros()))
        );
        assert_eq!(round.answered, round.posted);
    }
}

#[test]
fn a_beat_not_echoed_by_its_deadline_is_overdue_with_its_rounds() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (mut supervisor, mut responder) =
        pair(slot(), request, reply, PERIOD, table()).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    {
        // One round completes.
        let mut round = pin!(supervisor.round(&clock));
        assert!(round.as_mut().poll(&mut cx).is_pending());
        let mut serving = pin!(responder.serve());
        assert!(serving.as_mut().poll(&mut cx).is_pending());
        let Poll::Ready(Ok(round)) = round.as_mut().poll(&mut cx) else {
            panic!("the echoed beat completes its round");
        };
        assert_eq!(round.sequence, 1);
    }
    // The responder stops: the next beat stays posted past its deadline.
    let posted = clock.now();
    let mut round = pin!(supervisor.round(&clock));
    assert!(round.as_mut().poll(&mut cx).is_pending());
    let _ = clock.advance(Duration::from_micros(PERIOD.deadline().as_micros() - 1));
    assert!(round.as_mut().poll(&mut cx).is_pending());
    let _ = clock.advance(Duration::from_micros(1));
    let deadline = posted.checked_add(PERIOD.deadline()).expect("in range");
    let overdue = Overdue {
        sequence: 2,
        deadline,
        completed: 1,
    };
    assert_eq!(
        round.as_mut().poll(&mut cx),
        Poll::Ready(Err(LivenessError::Overdue(overdue)))
    );
    let micros = deadline.as_micros();
    assert_eq!(
        LivenessError::Overdue(overdue).diagnostic(),
        (
            FaultReason::LivenessOverdue,
            [2, micros as u32, (micros >> 32) as u32, 1]
        )
    );
}

#[test]
fn supervision_ends_at_the_first_overdue_beat() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    let (supervisor, _responder) =
        pair(slot(), request, reply, PERIOD, table()).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    let mut observed = 0_u32;
    {
        let mut supervising = pin!(supervisor.supervise(&clock, |_| observed += 1));
        assert!(supervising.as_mut().poll(&mut cx).is_pending());
        let _ = clock.advance_to_next_deadline();
        let Poll::Ready(LivenessError::Overdue(overdue)) = supervising.as_mut().poll(&mut cx)
        else {
            panic!("an unanswered first beat ends supervision");
        };
        assert_eq!((overdue.sequence, overdue.completed), (1, 0));
    }
    assert_eq!(observed, 0);
}

#[test]
fn a_supervisor_polled_outside_its_executor_is_refused() {
    let (request, _) = host::deferred_signal();
    let reply = host::foreign_signal();
    let (mut supervisor, _responder) =
        pair(slot(), request, reply, PERIOD, table()).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    let mut round = pin!(supervisor.round(&clock));
    assert!(matches!(
        round.as_mut().poll(&mut cx),
        Poll::Ready(Err(LivenessError::Foreign(_)))
    ));
}

#[test]
fn every_end_of_supervision_keeps_its_own_reason_and_words() {
    assert_eq!(
        LivenessError::Occupied { sequence: 5 }.diagnostic(),
        (FaultReason::LivenessBroken, [1, 5, 0, 0])
    );
    assert_eq!(
        LivenessError::WrongEcho {
            sequence: 5,
            echo: 4
        }
        .diagnostic(),
        (FaultReason::LivenessBroken, [2, 5, 4, 0])
    );
    assert_eq!(
        LivenessError::Time(oer_time::TimeOverflow).diagnostic(),
        (FaultReason::LivenessBroken, [3, 0, 0, 0])
    );
    assert_eq!(
        LivenessError::NotPosted { sequence: 5 }.diagnostic(),
        (FaultReason::LivenessBroken, [4, 5, 0, 0])
    );
}

#[test]
fn an_echo_taken_after_the_deadline_is_overdue_and_stays_overdue() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (mut supervisor, mut responder) =
        pair(slot(), request, reply, PERIOD, table()).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    let deadline = clock.deadline_after(PERIOD.deadline()).expect("in range");
    let overdue = LivenessError::Overdue(Overdue {
        sequence: 1,
        deadline,
        completed: 0,
    });
    {
        let mut round = pin!(supervisor.round(&clock));
        assert!(round.as_mut().poll(&mut cx).is_pending());
        // The responder echoes one microsecond past the deadline, before
        // the supervisor runs again.
        let _ = clock.advance(Duration::from_micros(PERIOD.deadline().as_micros() + 1));
        let mut serving = pin!(responder.serve());
        assert!(serving.as_mut().poll(&mut cx).is_pending());
        assert_eq!(round.as_mut().poll(&mut cx), Poll::Ready(Err(overdue)));
    }
    // Every later round reports the same overdue beat and posts nothing.
    let mut round = pin!(supervisor.round(&clock));
    assert_eq!(round.as_mut().poll(&mut cx), Poll::Ready(Err(overdue)));
}

/// One watched request of the supervisor's executor: its client's square.
#[derive(Debug, Eq, PartialEq)]
struct Ask(u32);

fn ask_slot() -> &'static RequestSlot<Ask, (), InPlace> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
}

/// A supervisor, its responder and one watched channel of its executor.
type Watched = (
    super::Supervisor<host::Deferred, host::Host, 2>,
    super::Responder<host::Deferred, host::Host>,
    super::super::PeerClient<Ask, host::Deferred, host::Host>,
    super::super::PeerServer<Ask, host::Deferred, host::Host>,
);

/// A supervisor whose responder echoes every beat and a watched channel of
/// the supervisor's executor (its client and server).
fn watched_channel() -> Watched {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (supervisor, responder) = pair(slot(), request, reply, PERIOD, table()).expect("fresh");
    let (asks, _) = host::deferred_signal();
    let (answers, _) = host::signal();
    let watch = supervisor.watch().expect("a free seat");
    let (client, server) =
        super::super::pair(ask_slot(), asks, answers, watch).expect("a fresh slot");
    (supervisor, responder, client, server)
}

/// Drive the supervisor and its responder for `steps` of `step`; the
/// supervisor's end and the clock then.
fn drive(
    clock: &VirtualClock<4>,
    supervising: &mut core::pin::Pin<&mut impl Future<Output = LivenessError>>,
    serving: &mut core::pin::Pin<
        &mut impl Future<Output = oer_espressif_interrupt_table::signal::ForeignContext>,
    >,
    step: Duration,
    steps: u32,
) -> Option<(LivenessError, oer_time::Instant)> {
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    for _ in 0..steps {
        for _ in 0..2 {
            if let Poll::Ready(error) = supervising.as_mut().poll(&mut cx) {
                return Some((error, clock.now()));
            }
            assert!(serving.as_mut().poll(&mut cx).is_pending());
        }
        let _ = clock.advance(step);
    }
    None
}

#[test]
fn a_request_never_waited_for_is_reported_at_its_deadline_while_beats_go_on() {
    let (supervisor, mut responder, mut client, _server) = watched_channel();
    let clock = VirtualClock::<4>::new();
    let deadline = clock
        .deadline_after(Duration::from_millis(70))
        .expect("in range");
    assert_eq!(client.post(Ask(3), deadline), Ok(()));
    let rounds = core::cell::Cell::new(0_u32);
    let mut supervising = pin!(supervisor.supervise(&clock, |_| rounds.set(rounds.get() + 1)));
    let mut serving = pin!(responder.serve());
    let Some((error, at)) = drive(
        &clock,
        &mut supervising,
        &mut serving,
        Duration::from_millis(1),
        200,
    ) else {
        panic!("the overdue request ends supervision");
    };
    assert_eq!(
        error,
        LivenessError::RequestOverdue(super::RequestOverdue {
            channel: 0,
            deadline
        })
    );
    assert_eq!(at, deadline, "reported at the deadline, not a round later");
    assert_eq!(
        rounds.get(),
        2,
        "the beats of 0 and 50 ms completed meanwhile"
    );
}

#[test]
fn an_abandoned_wait_ignored_expiry_and_late_answer_stay_overdue() {
    for case in 0..3 {
        let (supervisor, mut responder, mut client, mut server) = watched_channel();
        let clock = VirtualClock::<4>::new();
        let deadline = clock
            .deadline_after(Duration::from_millis(5))
            .expect("in range");
        let waker = waker();
        let mut cx = Context::from_waker(&waker);
        assert_eq!(client.post(Ask(4), deadline), Ok(()));
        match case {
            // A wait started, then dropped for good.
            0 => {
                let mut answering = pin!(client.answer(&clock));
                assert!(answering.as_mut().poll(&mut cx).is_pending());
            }
            // The expiry is returned and ignored.
            1 => {
                let _ = clock.advance(Duration::from_millis(5));
                let mut answering = pin!(client.answer(&clock));
                assert!(matches!(
                    answering.as_mut().poll(&mut cx),
                    Poll::Ready(Err(super::super::PeerError::Expired { .. }))
                ));
            }
            // The server answers after the deadline; the client takes it.
            _ => {
                let _ = clock.advance(Duration::from_millis(6));
                let mut serving = pin!(server.serve_one(|ask| ask.0 *= ask.0));
                assert_eq!(serving.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
                let mut answering = pin!(client.answer(&clock));
                assert!(matches!(
                    answering.as_mut().poll(&mut cx),
                    Poll::Ready(Err(super::super::PeerError::Expired { .. }))
                ));
            }
        }
        let mut supervising = pin!(supervisor.supervise(&clock, |_| ()));
        let mut serving = pin!(responder.serve());
        let Some((error, _)) = drive(
            &clock,
            &mut supervising,
            &mut serving,
            Duration::from_millis(1),
            20,
        ) else {
            panic!("case {case}: the overdue request ends supervision");
        };
        assert_eq!(
            error,
            LivenessError::RequestOverdue(super::RequestOverdue {
                channel: 0,
                deadline
            }),
            "case {case}"
        );
    }
}

#[test]
fn answers_taken_in_time_discharge_and_the_channel_is_used_again() {
    let (supervisor, mut responder, mut client, mut server) = watched_channel();
    let clock = VirtualClock::<4>::new();
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    let rounds = core::cell::Cell::new(0_u32);
    let mut supervising = pin!(supervisor.supervise(&clock, |_| rounds.set(rounds.get() + 1)));
    let mut serving = pin!(responder.serve());
    for value in [2, 3] {
        let deadline = clock
            .deadline_after(Duration::from_millis(5))
            .expect("in range");
        assert_eq!(client.post(Ask(value), deadline), Ok(()));
        let mut answer = pin!(server.serve_one(|ask| ask.0 *= ask.0));
        assert_eq!(answer.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        let mut answering = pin!(client.answer(&clock));
        assert_eq!(
            answering.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Ask(value * value)))
        );
        // Well past the discharged deadline: nothing is overdue.
        assert!(
            drive(
                &clock,
                &mut supervising,
                &mut serving,
                Duration::from_millis(1),
                40
            )
            .is_none()
        );
    }
    assert!(rounds.get() >= 2);
}

#[test]
fn a_new_obligation_wakes_the_sleeping_supervisor_for_its_earlier_deadline() {
    let (supervisor, mut responder, mut client, _server) = watched_channel();
    let clock = VirtualClock::<4>::new();
    let supervisor_wakes = Arc::new(Counting::default());
    let supervisor_waker = Waker::from(supervisor_wakes.clone());
    let mut supervisor_cx = Context::from_waker(&supervisor_waker);
    let other = waker();
    let mut cx = Context::from_waker(&other);
    let mut supervising = pin!(supervisor.supervise(&clock, |_| ()));
    let mut serving = pin!(responder.serve());
    // The first round completes; the supervisor sleeps until 50 ms.
    assert!(supervising.as_mut().poll(&mut supervisor_cx).is_pending());
    assert!(serving.as_mut().poll(&mut cx).is_pending());
    assert!(supervising.as_mut().poll(&mut supervisor_cx).is_pending());
    let before = supervisor_wakes.0.load(Ordering::Relaxed);
    let _ = clock.advance(Duration::from_millis(1));
    let deadline = clock
        .deadline_after(Duration::from_millis(4))
        .expect("in range");
    assert_eq!(client.post(Ask(5), deadline), Ok(()));
    assert!(
        supervisor_wakes.0.load(Ordering::Relaxed) > before,
        "the post woke the supervisor"
    );
    assert!(supervising.as_mut().poll(&mut supervisor_cx).is_pending());
    let _ = clock.advance(Duration::from_millis(4));
    assert_eq!(
        supervising.as_mut().poll(&mut supervisor_cx),
        Poll::Ready(LivenessError::RequestOverdue(super::RequestOverdue {
            channel: 0,
            deadline
        }))
    );
}

#[test]
fn a_table_serves_one_supervisor_and_issues_its_seats_once() {
    let table = table();
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (supervisor, _responder) = pair(slot(), request, reply, PERIOD, table).expect("fresh");
    assert!(pair(slot(), request, reply, PERIOD, table).is_none());
    let first = supervisor.watch().expect("seat 0");
    let second = supervisor.watch().expect("seat 1");
    assert_eq!((first.channel(), second.channel()), (0, 1));
    assert!(supervisor.watch().is_none(), "two seats only");
}

#[test]
fn a_supervised_overdue_request_records_its_channel_and_deadline() {
    let deadline = oer_time::Instant::from_micros(0x1_0000_0002);
    assert_eq!(
        LivenessError::RequestOverdue(super::RequestOverdue {
            channel: 1,
            deadline
        })
        .diagnostic(),
        (FaultReason::PeerUnanswered, [1, 2, 1, 2])
    );
}
