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

use super::{Beat, LivenessError, Overdue, Period, PeriodError, pair};

const PERIOD: Period = match Period::new(Duration::from_millis(50), Duration::from_millis(10)) {
    Ok(period) => period,
    Err(_) => panic!("a deadline within its period"),
};

fn slot() -> &'static RequestSlot<Beat, (), InPlace> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
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
    assert!(pair(slot, request, reply, PERIOD).is_some());
    assert!(pair(slot, request, reply, PERIOD).is_none());
}

#[test]
fn rounds_complete_once_per_period_while_the_responder_echoes() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (supervisor, mut responder) = pair(slot(), request, reply, PERIOD).expect("a fresh slot");
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
        pair(slot(), request, reply, PERIOD).expect("a fresh slot");
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
    let (supervisor, _responder) = pair(slot(), request, reply, PERIOD).expect("a fresh slot");
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
    let (mut supervisor, _responder) = pair(slot(), request, reply, PERIOD).expect("a fresh slot");
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
        pair(slot(), request, reply, PERIOD).expect("a fresh slot");
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
