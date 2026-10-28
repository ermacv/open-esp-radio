#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects
    )
)]

//! Deterministic virtual time for tests and simulations.
//!
//! [`VirtualClock`] implements [`oer_time::Clock`] and [`oer_time::Timer`]
//! for one instance: time stands still until its owner advances it, so a
//! test or a simulation decides exactly when every wait ends, and several
//! clocks in one process keep independent times. Advancing wakes every
//! pending wait; a wait whose deadline is still ahead registers again when
//! it is polled. There is no global time driver: code that needs time takes
//! a clock.
//!
//! [`SkipClock`] serves code under test that runs on one task and waits only
//! for its own deadlines: a wait's first poll moves time to its deadline
//! and completes it, so a blocking test runs a timed sequence without an
//! executor. Creating or dropping an unpolled wait leaves time unchanged.

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    task::{Poll, Waker},
};

use oer_time::{CLOCK_EPOCH_OFFSET, Clock, Duration, Instant, TimeOverflow, Timer};

/// Where [`VirtualClock::new`] and [`SkipClock::new`] start:
/// [`CLOCK_EPOCH_OFFSET`] past [`Instant::EPOCH`], as the image's clock
/// does, so code under test meets the 32-bit wrap at once.
pub const START: Instant = Instant::from_micros(CLOCK_EPOCH_OFFSET.as_micros());

/// The default number of tasks whose wakers a clock keeps apart; more
/// waiters are still served, at the cost of waking them all together.
pub const DEFAULT_WAITERS: usize = 8;

/// Virtual monotonic time, advanced explicitly by its owner.
///
/// `WAITERS` must be nonzero; every constructor checks this at compile time.
///
/// A read returns the time the owner last set, starting at the instant the
/// clock was made with: [`START`] by default. Waits end when an advance
/// reaches their deadline, or at once for a deadline already reached.
pub struct VirtualClock<const WAITERS: usize = DEFAULT_WAITERS> {
    now: Cell<Instant>,
    earliest: Cell<Option<Instant>>,
    waiters: [Cell<Option<Waker>>; WAITERS],
}

impl<const WAITERS: usize> Default for VirtualClock<WAITERS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const WAITERS: usize> VirtualClock<WAITERS> {
    /// A clock at [`START`], [`CLOCK_EPOCH_OFFSET`] past the epoch.
    pub const fn new() -> Self {
        Self::starting_at(START)
    }

    /// A clock at `start`, for a test that places time itself.
    pub const fn starting_at(start: Instant) -> Self {
        const {
            if WAITERS == 0 {
                panic!("a virtual clock needs at least one waker slot");
            }
        }
        Self {
            now: Cell::new(start),
            earliest: Cell::new(None),
            waiters: [const { Cell::new(None) }; WAITERS],
        }
    }

    /// Move time forward by `duration` and wake the pending waits.
    ///
    /// # Errors
    ///
    /// The new time would overflow; time does not move.
    pub fn advance(&self, duration: Duration) -> Result<Instant, TimeOverflow> {
        let at = self.now.get().checked_add(duration).ok_or(TimeOverflow)?;
        self.set(at);
        Ok(at)
    }

    /// Move time forward to `at` and wake the pending waits. An instant
    /// already passed leaves time where it is: the clock never goes back.
    pub fn advance_to(&self, at: Instant) {
        self.set(at.max(self.now.get()));
    }

    /// The earliest deadline a wait still pending asked for since the last
    /// advance; `None` when no wait has asked since, for instance before
    /// the woken waits have been polled again.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.earliest.get()
    }

    /// Advance to [`Self::next_deadline`], returning it; `None` leaves time
    /// where it is.
    pub fn advance_to_next_deadline(&self) -> Option<Instant> {
        let deadline = self.earliest.get()?;
        self.advance_to(deadline);
        Some(deadline)
    }

    fn set(&self, at: Instant) {
        self.now.set(at);
        self.earliest.set(None);
        Self::wake(self.take_waiters());
    }

    fn take_waiters(&self) -> [Option<Waker>; WAITERS] {
        self.waiters.each_ref().map(Cell::take)
    }

    fn wake(waiters: [Option<Waker>; WAITERS]) {
        // All slots are available before any user callback can reenter.
        for waker in waiters.into_iter().flatten() {
            waker.wake();
        }
    }

    fn register(&self, waker: Waker) {
        for slot in &self.waiters {
            let registered = slot.take();
            let duplicate = registered
                .as_ref()
                .is_some_and(|saved| saved.will_wake(&waker));
            slot.set(registered);
            if duplicate {
                return;
            }
        }
        for slot in &self.waiters {
            match slot.take() {
                None => {
                    slot.set(Some(waker));
                    return;
                }
                registered => slot.set(registered),
            }
        }
        let waiting = self.take_waiters();
        // Construction const-checks WAITERS > 0. Install the new waker
        // before callbacks: a reentrant advance must wake it as well.
        if let Some(first) = self.waiters.first() {
            first.set(Some(waker));
        }
        Self::wake(waiting);
    }
}

impl<const WAITERS: usize> Clock for VirtualClock<WAITERS> {
    fn now(&self) -> Instant {
        self.now.get()
    }
}

impl<const WAITERS: usize> Timer for VirtualClock<WAITERS> {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        poll_fn(move |context| {
            if self.now.get() >= deadline {
                return Poll::Ready(());
            }
            // Cloning may run a user callback too. Recheck time before
            // publishing the deadline and its registration.
            let waker = context.waker().clone();
            if self.now.get() >= deadline {
                return Poll::Ready(());
            }
            self.earliest.set(Some(
                self.earliest
                    .get()
                    .map_or(deadline, |earliest| earliest.min(deadline)),
            ));
            self.register(waker);
            Poll::Pending
        })
    }
}

/// Virtual time that skips ahead to every deadline waited for.
///
/// A wait's first poll moves time to its deadline, unless time is already
/// past it, and completes the wait. Creating or dropping an unpolled wait
/// has no effect; time otherwise moves only when the owner advances it.
/// Use it where one task waits on its own deadlines; tasks that wait for
/// each other need [`VirtualClock`].
#[derive(Debug)]
pub struct SkipClock {
    now: Cell<Instant>,
}

impl Default for SkipClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SkipClock {
    /// A clock at [`START`], [`CLOCK_EPOCH_OFFSET`] past the epoch.
    pub const fn new() -> Self {
        Self::starting_at(START)
    }

    /// A clock at `start`, for a test that places time itself.
    pub const fn starting_at(start: Instant) -> Self {
        Self {
            now: Cell::new(start),
        }
    }

    /// Move time forward to `at`. An instant already passed leaves time
    /// where it is: the clock never goes back.
    pub fn advance_to(&self, at: Instant) {
        self.now.set(at.max(self.now.get()));
    }
}

impl Clock for SkipClock {
    fn now(&self) -> Instant {
        self.now.get()
    }
}

impl Timer for SkipClock {
    async fn wait_until(&self, deadline: Instant) {
        self.advance_to(deadline);
    }
}

#[cfg(test)]
mod tests;
