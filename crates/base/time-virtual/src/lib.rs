#![no_std]
#![forbid(unsafe_code)]

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
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    task::Poll,
};

use embassy_sync::waitqueue::MultiWakerRegistration;
use oer_time::{Clock, Duration, Instant, TimeOverflow, Timer};

/// The default number of tasks whose wakers a clock keeps apart; more
/// waiters are still served, at the cost of waking them all together.
pub const DEFAULT_WAITERS: usize = 8;

/// Virtual monotonic time, advanced explicitly by its owner.
///
/// A read returns the time the owner last set, starting at the instant the
/// clock was made with. Waits end when an advance reaches their deadline,
/// or at once for a deadline already reached.
pub struct VirtualClock<const WAITERS: usize = DEFAULT_WAITERS> {
    now: Cell<Instant>,
    earliest: Cell<Option<Instant>>,
    waiters: RefCell<MultiWakerRegistration<WAITERS>>,
}

impl<const WAITERS: usize> Default for VirtualClock<WAITERS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const WAITERS: usize> VirtualClock<WAITERS> {
    /// A clock at [`Instant::EPOCH`].
    pub const fn new() -> Self {
        Self::starting_at(Instant::EPOCH)
    }

    /// A clock at `start`.
    pub const fn starting_at(start: Instant) -> Self {
        Self {
            now: Cell::new(start),
            earliest: Cell::new(None),
            waiters: RefCell::new(MultiWakerRegistration::new()),
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
        self.waiters.borrow_mut().wake();
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
            self.earliest.set(Some(
                self.earliest
                    .get()
                    .map_or(deadline, |earliest| earliest.min(deadline)),
            ));
            self.waiters.borrow_mut().register(context.waker());
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
    /// A clock at [`Instant::EPOCH`].
    pub const fn new() -> Self {
        Self::starting_at(Instant::EPOCH)
    }

    /// A clock at `start`.
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
