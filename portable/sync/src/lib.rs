#![no_std]
#![forbid(unsafe_code)]

//! Notification across task priorities, without a lock or a critical section.
//!
//! An ordinary `AtomicWaker` may self-schedule a registering consumer while a
//! preempted producer holds WAKING. [`PriorityWaker`] binds the other slot;
//! the lower-priority producer can then resume and complete its notification.

use atomic_waker::AtomicWaker;
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::Waker,
};

struct Slot {
    waker: AtomicWaker,
    waking: AtomicUsize,
}
impl Slot {
    const fn new() -> Self {
        Self {
            waker: AtomicWaker::new(),
            waking: AtomicUsize::new(0),
        }
    }

    // Keep obsolete-waker destruction separate from registration's wake
    // callbacks so each indirect call retains its own Waker operation scope.
    #[inline(never)]
    fn release_if_idle(&self) {
        if self.waking.load(Ordering::Acquire) == 0 {
            drop(self.waker.take());
        }
    }
}

/// Notification for one serialized consumer that may preempt its producers.
///
/// On one hart, the consumer may run at the same or a higher priority than
/// every producer. Registration must not run concurrently with itself. Nested
/// notifications are supported. Cross-core access remains memory safe, but
/// the no-self-scheduling progress guarantee requires this single-hart ordering.
/// This is not an IRQ-to-task interface: handlers use typed interrupt signals.
///
/// Publish the input before [`wake`](Self::wake), then register before checking
/// that input with Acquire ordering. Notifications are hints, not stored events;
/// a notification before the first registration is found by the input check.
/// A waker clone/drop callback must not reenter registration.
pub struct PriorityWaker {
    slots: [Slot; 2],
    active: AtomicBool,
}
impl PriorityWaker {
    /// Create an empty notification cell.
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; 2],
            active: AtomicBool::new(false),
        }
    }
    fn slot(&self, index: bool) -> &Slot {
        if index {
            &self.slots[1]
        } else {
            &self.slots[0]
        }
    }
    /// Bind the next wait to `waker`, without waking it on replacement.
    ///
    /// An interrupted producer keeps its old registration until it finishes.
    /// A subsequent registration releases that obsolete waker when idle.
    pub fn register(&self, waker: &Waker) {
        let mut selected = self.active.load(Ordering::Acquire);
        if self.slot(selected).waking.load(Ordering::Acquire) != 0 {
            selected = !selected;
        }
        // One serialized consumer preempts the producers; at most one slot
        // can be held by a suspended producer here. Registering in its peer
        // cannot observe AtomicWaker's WAKING state and enqueue the consumer.
        self.slot(selected).waker.register(waker);
        self.active.store(selected, Ordering::Release);
        let inactive = self.slot(!selected);
        // An interrupted producer has already published its input, checked
        // after registration. Keep its old waker only until it finishes.
        inactive.release_if_idle();
    }
    fn begin_wake(&self) -> Waking<'_> {
        let slot = self.slot(self.active.load(Ordering::Acquire));
        // A wake can nest inside a suspended lower-priority wake; its completion
        // must not release the outer producer's slot.
        slot.waking.fetch_add(1, Ordering::AcqRel);
        Waking(slot)
    }
    /// Notify the current consumer after publishing its input.
    pub fn wake(&self) {
        self.begin_wake().fire();
    }
}
struct Waking<'a>(&'a Slot);
impl Waking<'_> {
    fn fire(&self) {
        self.0.waker.wake();
    }
}
impl Drop for Waking<'_> {
    fn drop(&mut self) {
        self.0.waking.fetch_sub(1, Ordering::Release);
    }
}


impl Default for PriorityWaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
