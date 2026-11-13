#![cfg_attr(not(feature = "loom"), no_std)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
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

//! A cell holding one task's waker, shared with the producers that wake it.
//!
//! [`AtomicWaker`] carries the state machine of the `atomic-waker` crate, 1.1.2
//! (MIT OR Apache-2.0, derived from `futures` and Tokio), without its `unwrap`
//! and debug assertions. The wake a registration owes after a concurrent wake
//! goes through the waker the registration holds as its argument, so no path
//! depends on finding one in the cell.
//!
//! A call makes a fixed number of atomic operations and at most one waker call
//! and has no retry loop of its own. That bound is algorithmic: an atomic
//! operation may be a retried reservation in machine code.
//!
//! Registration wakes its own task when it finds a wake in progress, so a
//! consumer above a preempted producer must use `oer_portable_sync::PriorityWaker`.

mod cell;

use core::fmt;
use core::mem::ManuallyDrop;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use core::task::Waker;

use cell::{AtomicUsize, UnsafeCell};

/// No registration or wake in progress.
const WAITING: usize = 0;
/// A registration holds the cell.
const REGISTERING: usize = 0b01;
/// A wake holds the cell, or came during a registration.
const WAKING: usize = 0b10;

/// One task's waker.
///
/// The cell holds at most one waker. Registrations do not run concurrently
/// with each other: a registration that finds another in progress does nothing.
/// Producers publish their input before [`wake`](Self::wake); the consumer
/// registers before it checks that input. A waker's clone and drop run with
/// the cell held, so they must not call into it.
///
/// The cell's one destructor ([`Drop`]) drops the waker it holds: the field
/// has no drop glue of its own, so no type that holds a cell inlines a
/// waker's drop, and every drop of a cell's waker in an image is that one
/// function.
pub struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<ManuallyDrop<Option<Waker>>>,
}

#[allow(unsafe_code, reason = "the waker is reached only under the state word")]
// SAFETY: a `Waker` is `Send + Sync`, and every access to `waker` holds
// `REGISTERING` or `WAKING` won from `WAITING`, which exclude each other.
unsafe impl Sync for AtomicWaker {}

impl AtomicWaker {
    /// The empty cell.
    #[cfg(not(feature = "loom"))]
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(ManuallyDrop::new(None)),
        }
    }

    /// The empty cell. The model's atomics have no constant constructor.
    #[cfg(feature = "loom")]
    pub fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(ManuallyDrop::new(None)),
        }
    }

    /// Make `waker` the one the next [`wake`](Self::wake) calls.
    ///
    /// A wake in progress, or one that comes during this call, wakes `waker`
    /// at once: the caller then checks its input and may look again.
    #[allow(unsafe_code, reason = "the winning exchange holds the waker")]
    pub fn register(&self, waker: &Waker) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Acquire, Acquire)
            .unwrap_or_else(|state| state)
        {
            WAITING => {
                // SAFETY: the exchange from `WAITING` holds the cell until the
                // state returns to `WAITING`: a wake only sets `WAKING` and
                // leaves the waker alone while `REGISTERING` is set.
                unsafe { self.store(waker) };
                if self
                    .state
                    .compare_exchange(REGISTERING, WAITING, AcqRel, Acquire)
                    .is_err()
                {
                    // A wake came during the registration and did nothing. It
                    // owes a wake to the task `waker` names, which the stored
                    // waker names too: the cell is emptied as that wake would
                    // have emptied it, and `waker` carries it.
                    // SAFETY: `REGISTERING` is still set, as above.
                    let stored = unsafe { self.take_stored() };
                    self.state.swap(WAITING, AcqRel);
                    waker.wake_by_ref();
                    drop(stored);
                }
            }
            WAKING => waker.wake_by_ref(),
            // Another registration holds the cell; the two do not run
            // concurrently by this type's contract. Memory stays safe.
            _ => {}
        }
    }

    /// Wake the registered task and empty the cell, if a task is registered.
    pub fn wake(&self) {
        if let Some(waker) = self.take() {
            waker.wake();
        }
    }

    /// Empty the cell and return its waker. While a registration is in
    /// progress this takes nothing and that registration wakes its task.
    #[allow(unsafe_code, reason = "the winning fetch holds the waker")]
    pub fn take(&self) -> Option<Waker> {
        match self.state.fetch_or(WAKING, AcqRel) {
            WAITING => {
                // SAFETY: the fetch moved the state from `WAITING` to `WAKING`,
                // so this context holds the cell until it clears `WAKING`.
                let waker = unsafe { self.take_stored() };
                self.state.fetch_and(!WAKING, Release);
                waker
            }
            _ => None,
        }
    }

    /// Store `waker`, unless the cell holds one of the same task.
    ///
    /// # Safety
    /// The caller holds the cell.
    #[allow(unsafe_code, reason = "the caller holds the cell")]
    unsafe fn store(&self, waker: &Waker) {
        self.waker.with_mut(|stored| {
            // SAFETY: the caller holds the cell, by this function's contract.
            let stored: &mut Option<Waker> = unsafe { &mut *stored };
            match stored {
                Some(old) if old.will_wake(waker) => {}
                _ => *stored = Some(waker.clone()),
            }
        });
    }

    /// Empty the cell.
    ///
    /// # Safety
    /// The caller holds the cell.
    #[allow(unsafe_code, reason = "the caller holds the cell")]
    unsafe fn take_stored(&self) -> Option<Waker> {
        self.waker.with_mut(|stored| {
            // SAFETY: the caller holds the cell, by this function's contract.
            let stored: &mut Option<Waker> = unsafe { &mut *stored };
            stored.take()
        })
    }
}

impl Drop for AtomicWaker {
    /// Drop the waker the cell holds, if any: the one place a cell's waker
    /// is dropped when the cell goes.
    #[inline(never)]
    #[allow(unsafe_code, reason = "the destructor holds the cell exclusively")]
    fn drop(&mut self) {
        // SAFETY: `&mut self` excludes every registration and wake, so this
        // context holds the cell; the field is never read after this.
        let waker = unsafe { self.take_stored() };
        drop(waker);
    }
}

impl Default for AtomicWaker {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AtomicWaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AtomicWaker")
    }
}

#[cfg(all(test, feature = "loom"))]
mod model;
#[cfg(all(test, not(feature = "loom")))]
mod tests;
