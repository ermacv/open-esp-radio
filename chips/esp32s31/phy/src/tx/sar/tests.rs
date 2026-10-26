use super::*;
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use oer_time::{Clock, Instant};
use oer_time_virtual::VirtualClock;
use std::{
    boxed::Box,
    cell::{Cell, RefCell},
};

#[test]
fn readiness_beyond_the_old_settle_can_arrive_within_the_deadline() {
    let clock: VirtualClock = VirtualClock::default();
    let started = clock.now();
    let reads = Cell::new(0);
    let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || {
        reads.set(reads.get() + 1);
        clock.now().saturating_duration_since(started).as_micros() >= 27
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(reads.get(), 1);
    assert_eq!(
        clock.next_deadline(),
        started.checked_add(Duration::from_micros(1))
    );
    clock.advance(Duration::from_micros(27)).unwrap();
    assert_eq!(
        future.as_mut().poll(&mut cx),
        Poll::Ready(Readiness {
            ready: true,
            elapsed_micros: 27,
            observations: 2,
        })
    );
}

#[test]
fn a_ready_sample_taken_after_the_deadline_succeeds() {
    let clock: VirtualClock = VirtualClock::default();
    let started = clock.now();
    let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || {
        clock.now().saturating_duration_since(started).as_micros() >= 51
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    clock.advance(Duration::from_micros(51)).unwrap();
    assert_eq!(
        future.as_mut().poll(&mut cx),
        Poll::Ready(Readiness {
            ready: true,
            elapsed_micros: 51,
            observations: 2,
        })
    );
}

#[test]
fn ready_on_the_first_sample_succeeds_without_waiting() {
    let clock: VirtualClock = VirtualClock::default();
    let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || true));
    assert_eq!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Readiness {
            ready: true,
            elapsed_micros: 0,
            observations: 1,
        })
    );
    assert_eq!(clock.next_deadline(), None);
}

#[test]
fn never_ready_fails_at_the_deadline() {
    let clock: VirtualClock = VirtualClock::default();
    let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || false));
    let mut cx = Context::from_waker(Waker::noop());
    let mut polls = 0_u16;
    let readiness = loop {
        polls += 1;
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(readiness) => break readiness,
            Poll::Pending => {
                clock.advance_to_next_deadline().unwrap();
            }
        }
    };
    assert_eq!(
        readiness,
        Readiness {
            ready: false,
            elapsed_micros: 50,
            observations: 51,
        }
    );
    assert_eq!(polls, 51);
    assert_eq!(clock.next_deadline(), None);
}

#[test]
fn the_limit_accepts_ready_but_never_waits_again_for_unready_hardware() {
    for ready_at_limit in [false, true] {
        let clock: VirtualClock = VirtualClock::default();
        let started = clock.now();
        let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || {
            ready_at_limit && clock.now().saturating_duration_since(started).as_micros() >= 50
        }));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        clock.advance(Duration::from_micros(50)).unwrap();
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Readiness {
                ready: ready_at_limit,
                elapsed_micros: 50,
                observations: 2,
            })
        );
        assert_eq!(clock.next_deadline(), None);
    }
}

#[test]
fn the_sample_closure_keeps_its_exclusive_owner_while_suspended() {
    let clock: VirtualClock = VirtualClock::default();
    let owner = RefCell::new(());
    let physical = owner.borrow_mut();
    let timer = &clock;
    let started = clock.now();
    let mut future = Box::pin(wait(timer, Duration::from_micros(50), move || {
        let _held = &physical;
        timer.now() > started
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    assert!(owner.try_borrow_mut().is_err());
    clock.advance_to_next_deadline().unwrap();
    assert!(matches!(
        future.as_mut().poll(&mut cx),
        Poll::Ready(Readiness { ready: true, .. })
    ));
    drop(future);
    assert!(owner.try_borrow_mut().is_ok());
}

#[test]
fn an_unrepresentable_deadline_touches_no_hardware() {
    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 10));
    let mut future = Box::pin(wait(&clock, Duration::from_micros(50), || -> bool {
        panic!("an invalid deadline must not touch hardware")
    }));
    assert_eq!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Readiness {
            ready: false,
            elapsed_micros: 0,
            observations: 0,
        })
    );
}
