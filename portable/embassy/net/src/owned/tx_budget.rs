//! Admission credit follows a software packet through queueing and radio retention.

use core::{cell::RefCell, task::Waker};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};

struct State {
    outstanding: usize,
    sender: WakerRegistration,
}

pub(super) struct TxBudget<M: RawMutex> {
    capacity: usize,
    state: Mutex<M, RefCell<State>>,
}

impl<M: RawMutex> TxBudget<M> {
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(RefCell::new(State {
                outstanding: 0,
                sender: WakerRegistration::new(),
            })),
        }
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        self.state
            .lock(|state| state.borrow_mut().sender.register(waker));
    }

    pub(super) fn is_full(&self) -> bool {
        self.state
            .lock(|state| state.borrow().outstanding == self.capacity)
    }

    pub(super) fn try_admit(&self) -> bool {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.outstanding == self.capacity {
                return false;
            }
            state.outstanding += 1;
            true
        })
    }

    /// Transfer a queue's already admitted credit to its radio-side owner.
    pub(super) fn claim(&self) -> TxCredit<'_, M> {
        TxCredit(self)
    }
}

/// Non-cloneable credit; stored after the packet so packet destruction happens
/// before the producer is told that another admission is possible.
pub(super) struct TxCredit<'resources, M: RawMutex>(&'resources TxBudget<M>);

impl<M: RawMutex> Drop for TxCredit<'_, M> {
    fn drop(&mut self) {
        self.0.state.lock(|state| {
            let mut state = state.borrow_mut();
            let was_full = state.outstanding == self.0.capacity;
            state.outstanding -= 1;
            if was_full {
                state.sender.wake();
            }
        });
    }
}
