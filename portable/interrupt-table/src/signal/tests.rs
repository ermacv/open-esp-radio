extern crate std;

use core::{
    cell::Cell,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use super::*;

std::thread_local! {
    static PENDS: Cell<u32> = const { Cell::new(0) };
    static RUNNING: Cell<bool> = const { Cell::new(true) };
}

/// A pend that only counts: the test plays the executor's handler.
fn count_pend(_: &'static IrqSignal) {
    PENDS.with(|pends| pends.set(pends.get() + 1));
}

fn running() -> bool {
    RUNNING.with(Cell::get)
}

fn pends() -> u32 {
    PENDS.with(Cell::get)
}

/// A signal and its executor, leaked for the test thread alone.
fn executor() -> SignalExecutor {
    let list = Box::leak(Box::new(SignalList::new()));
    // SAFETY: the test thread is the one context; the pend only counts.
    #[allow(unsafe_code, reason = "the test executor's contract")]
    unsafe {
        SignalExecutor::new(count_pend, running, list)
    }
}

fn signal(executor: SignalExecutor) -> &'static IrqSignal {
    Box::leak(Box::new(IrqSignal::new(executor)))
}

/// A waker that counts its wakes and runs `on_wake` in them.
struct Counting {
    wakes: Cell<u32>,
    on_wake: Option<&'static IrqSignal>,
}

// The tests wake on their one thread only.
#[allow(unsafe_code, reason = "a single-threaded test waker")]
// SAFETY: every test touches its waker from its one thread.
unsafe impl Send for Counting {}
#[allow(unsafe_code, reason = "a single-threaded test waker")]
// SAFETY: as `Send`.
unsafe impl Sync for Counting {}

impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wakes.set(self.wakes.get() + 1);
        // A fact raised while the executor walks its signals.
        if let Some(signal) = self.on_wake {
            signal.raise(0b100);
        }
    }
}

fn counting(on_wake: Option<&'static IrqSignal>) -> (Arc<Counting>, Waker) {
    let counting = Arc::new(Counting {
        wakes: Cell::new(0),
        on_wake,
    });
    (counting.clone(), Waker::from(counting))
}

#[test]
fn raised_facts_coalesce_into_one_pend_and_one_wake() {
    let executor = executor();
    let signal = signal(executor);
    let (counting, waker) = counting(None);
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    let before = pends();
    // Two raises before the executor runs: one new-fact edge.
    assert!(signal.raise(0b01));
    assert!(signal.raise(0b10));
    assert!(!signal.raise(0b01));
    assert_eq!(pends() - before, 2);
    assert_eq!(signal.has_pending(), Ok(true));
    executor.service();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b11)));
    assert_eq!(signal.has_pending(), Ok(false));
    // Without facts the executor wakes nobody.
    executor.service();
    assert_eq!(counting.wakes.get(), 1);
}

#[test]
fn a_fact_raised_while_the_executor_walks_its_signals_is_not_lost() {
    let executor = executor();
    let signal = signal(executor);
    let (counting, waker) = counting(Some(signal));
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    signal.raise(0b01);
    let before = pends();
    // The wake raises another fact: it lands in the swapped word, is new
    // and pends the executor again.
    executor.service();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(pends() - before, 1);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b101)));
}

#[test]
fn the_executor_wakes_every_signal_its_tasks_polled() {
    let executor = executor();
    let first = signal(executor);
    let second = signal(executor);
    let (one, waker_one) = counting(None);
    let (two, waker_two) = counting(None);
    assert!(
        first
            .poll_take(&mut Context::from_waker(&waker_one))
            .is_pending()
    );
    assert!(
        second
            .poll_take(&mut Context::from_waker(&waker_two))
            .is_pending()
    );
    first.raise(1);
    second.raise(1);
    executor.service();
    assert_eq!((one.wakes.get(), two.wakes.get()), (1, 1));
}

#[test]
fn outside_its_executor_a_signal_only_raises() {
    let executor = executor();
    let signal = signal(executor);
    let (counting, waker) = counting(None);
    RUNNING.with(|running| running.set(false));
    assert_eq!(
        signal.poll_take(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(ForeignContext))
    );
    assert!(signal.raise(1));
    assert_eq!(signal.take(), Err(ForeignContext));
    executor.service();
    RUNNING.with(|running| running.set(true));
    assert_eq!(counting.wakes.get(), 0);
    // The fact waited for the executor's context.
    assert_eq!(signal.take(), Ok(1));
}

#[test]
fn the_host_executor_wakes_at_the_raise() {
    let signal = host::signal();
    let (counting, waker) = counting(None);
    let mut wait = pin!(signal.wait());
    let mut cx = Context::from_waker(&waker);
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    signal.raise(0b1);
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(Ok(0b1)));
}

#[test]
fn a_pending_poll_leaves_the_facts_for_the_take() {
    let executor = executor();
    let signal = signal(executor);
    let (counting, waker) = counting(None);
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_pending(&mut cx), Poll::Pending);
    signal.raise(0b10);
    executor.service();
    assert_eq!(counting.wakes.get(), 1);
    // Ready until the task takes the fact, however often it polls.
    assert_eq!(signal.poll_pending(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(signal.poll_pending(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(signal.take(), Ok(0b10));
    assert_eq!(signal.poll_pending(&mut cx), Poll::Pending);
    RUNNING.with(|running| running.set(false));
    assert_eq!(
        signal.poll_pending(&mut cx),
        Poll::Ready(Err(ForeignContext))
    );
    RUNNING.with(|running| running.set(true));
}
