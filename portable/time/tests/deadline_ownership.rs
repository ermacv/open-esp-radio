use core::{cell::Cell, num::NonZeroU32};
use oer_time::{
    Clock, Duration,
    deadline::{Backend, DeadlineBudget, DeadlineError, Watchdog},
};
use oer_time_virtual::VirtualClock;

#[derive(Default)]
struct State {
    armed: Cell<bool>,
    arms: Cell<u32>,
    disarms: Cell<u32>,
}
struct Hardware<'a> {
    clock: &'a VirtualClock<1>,
    state: &'a State,
}
impl Backend for Hardware<'_> {
    fn now_micros(&self) -> u64 {
        self.clock.now().as_micros()
    }
    fn arm(&mut self, _: u32) {
        self.state.armed.set(true);
        self.state.arms.set(self.state.arms.get() + 1);
    }
    fn disarm(&mut self) {
        self.state.armed.set(false);
        self.state.disarms.set(self.state.disarms.get() + 1);
    }
}
fn budget() -> DeadlineBudget {
    DeadlineBudget::from_micros(NonZeroU32::new(100).expect("nonzero budget"))
}
#[test]
fn completion_returns_the_owner_for_the_next_obligation() {
    let clock = VirtualClock::new();
    let state = State::default();
    let service = Watchdog::new(Hardware {
        clock: &clock,
        state: &state,
    });
    let lease = service.arm(budget()).expect("first obligation");
    clock
        .advance(Duration::from_micros(99))
        .expect("virtual time");
    let service = lease
        .complete()
        .expect("timely completion returns the owner");
    assert!(!state.armed.get());
    let lease = service.arm(budget()).expect("next obligation");
    assert_eq!(state.arms.get(), 2);
    lease.complete().expect("next completion");
    assert_eq!(state.disarms.get(), 2);
}
#[test]
fn cancellation_and_forgetting_leave_hardware_armed() {
    for forget in [false, true] {
        let clock = VirtualClock::new();
        let state = State::default();
        let service = Watchdog::new(Hardware {
            clock: &clock,
            state: &state,
        });
        let lease = service.arm(budget()).expect("armed");
        if forget {
            core::mem::forget(lease);
        } else {
            drop(lease);
        }
        clock
            .advance(Duration::from_micros(100))
            .expect("virtual time");
        assert!(state.armed.get());
        assert_eq!(state.arms.get(), 1);
        assert_eq!(state.disarms.get(), 0);
    }
}
#[test]
fn equality_and_late_completion_retain_the_armed_owner() {
    for elapsed in [100, 101] {
        let clock = VirtualClock::new();
        let state = State::default();
        let service = Watchdog::new(Hardware {
            clock: &clock,
            state: &state,
        });
        let lease = service.arm(budget()).expect("armed");
        clock
            .advance(Duration::from_micros(elapsed))
            .expect("virtual time");
        let (lease, error) = lease.complete().expect_err("expired completion");
        assert_eq!(error, DeadlineError::Expired);
        assert!(state.armed.get());
        drop(lease);
        assert_eq!(state.disarms.get(), 0);
    }
}
