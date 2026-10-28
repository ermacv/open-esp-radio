//! Admission credit follows a software packet through queueing and radio retention.

use core::{cell::Cell, task::Waker};

use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};

use super::wake_replaced;

struct State {
    outstanding: usize,
    sender: Option<Waker>,
}

impl State {
    const fn new() -> Self {
        Self {
            outstanding: 0,
            sender: None,
        }
    }
}

pub(super) struct TxBudget<M: RawMutex> {
    capacity: usize,
    state: Mutex<M, Cell<State>>,
}

impl<M: RawMutex> TxBudget<M> {
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(Cell::new(State::new())),
        }
    }

    // Callbacks, including Waker clone/drop, must run outside this closure.
    fn with_state<R>(&self, action: impl FnOnce(&mut State) -> R) -> R {
        self.state.lock(|cell| {
            let mut state = cell.replace(State::new());
            let result = action(&mut state);
            cell.set(state);
            result
        })
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        let next = waker.clone();
        let previous = self.with_state(|state| state.sender.replace(next));
        wake_replaced(previous, waker);
    }

    pub(super) fn is_full(&self) -> bool {
        self.with_state(|state| state.outstanding == self.capacity)
    }

    pub(super) fn try_admit(&self) -> bool {
        self.with_state(|state| {
            let Some(next) = state
                .outstanding
                .checked_add(1)
                .filter(|next| *next <= self.capacity)
            else {
                return false;
            };
            state.outstanding = next;
            true
        })
    }

    /// Transfer one already admitted credit, either from a dequeued packet or
    /// back from a failed insertion. Only these two queue paths mint a credit;
    /// each admitted packet therefore releases exactly once.
    pub(super) fn claim(&self) -> TxCredit<'_, M> {
        TxCredit(self)
    }
}

/// Non-cloneable credit; stored after the packet so packet destruction happens
/// before the producer is told that another admission is possible.
pub(super) struct TxCredit<'resources, M: RawMutex>(&'resources TxBudget<M>);

impl<M: RawMutex> Drop for TxCredit<'_, M> {
    fn drop(&mut self) {
        let wake = self.0.with_state(|state| {
            let was_full = state.outstanding == self.0.capacity;
            // Each affine credit discharges a successful admission exactly once.
            // No API can construct or clone a credit, so this never wraps.
            state.outstanding = state.outstanding.wrapping_sub(1);
            if was_full { state.sender.take() } else { None }
        });
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
