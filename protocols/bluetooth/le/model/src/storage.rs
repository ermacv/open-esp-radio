//! Local ownership transfers. No user callback runs while a collection is out.
use alloc::rc::Rc;
use core::{cell::Cell, task::Waker};

pub(crate) struct Local<T>(Cell<T>);
impl<T> Local<T> {
    pub const fn new(value: T) -> Self {
        Self(Cell::new(value))
    }
}
impl<T: Default> Local<T> {
    // Callers only move owners or inspect plain values here. Return removed
    // owners before dropping them, and clone/wake user wakers outside this call.
    pub fn with<R>(&self, operation: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.0.take();
        let result = operation(&mut value);
        self.0.set(value);
        result
    }
}

pub(crate) struct WakerSlot(Cell<Option<Rc<Waker>>>);
impl WakerSlot {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }
    pub fn register(&self, waker: &Waker) {
        // Cloning and dropping a Waker can invoke user code. Publish the new
        // registration before dropping the old one, with no borrowed storage.
        let next = Rc::new(waker.clone());
        let previous = self.0.replace(Some(next));
        drop(previous);
    }
    fn snapshot(&self) -> Option<Rc<Waker>> {
        let current = self.0.take();
        let snapshot = current.clone();
        self.0.set(current);
        snapshot
    }
    pub fn wake(&self) {
        if let Some(waker) = self.snapshot() {
            waker.wake_by_ref();
        }
    }
    pub fn clear(&self) {
        let previous = self.0.take();
        drop(previous);
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_some(&self) -> bool {
        self.snapshot().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, task::Wake};

    #[test]
    fn waking_can_replace_its_registration_without_a_borrow_conflict_or_lost_waker() {
        // A function-local TLS key bridges Wake's Send + Sync requirement to
        // this test thread's owned local slot; no state is shared across tests.
        std::thread_local! { static SLOT: WakerSlot = const { WakerSlot::new() }; }
        struct Replace;
        impl Wake for Replace {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                SLOT.with(|slot| {
                    slot.clear();
                    slot.register(Waker::noop());
                });
            }
        }
        SLOT.with(|slot| {
            slot.register(&Waker::from(Arc::new(Replace)));
            slot.wake();
            assert!(
                slot.is_some(),
                "the callback's replacement remains registered"
            );
            slot.wake();
            slot.clear();
            assert!(!slot.is_some());
        });
    }
    #[test]
    fn dropping_a_replaced_waker_observes_the_new_registration() {
        std::thread_local! { static SLOT: WakerSlot = const { WakerSlot::new() }; }
        struct OnDrop;
        impl Drop for OnDrop {
            fn drop(&mut self) {
                SLOT.with(|slot| {
                    assert!(
                        slot.is_some(),
                        "replacement is published before the old destructor"
                    );
                    slot.clear();
                });
            }
        }
        impl Wake for OnDrop {
            fn wake(self: Arc<Self>) {}
        }
        SLOT.with(|slot| {
            slot.register(&Waker::from(Arc::new(OnDrop)));
            slot.register(Waker::noop());
            assert!(!slot.is_some(), "the destructor's clear is retained");
        });
    }
}
