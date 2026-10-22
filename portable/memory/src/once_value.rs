//! First-writer publication without waiting for an interrupted writer.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

/// One value retained by the first writer.
///
/// A higher-priority reader never waits for a preempted writer: `get` returns
/// `None` until publication completes, while `is_requested` already reports
/// the request. The caller supplies its own wake after `set`. Reset requires
/// exclusive access, after every endpoint of the previous epoch ended.
pub struct OnceValue<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

#[allow(
    unsafe_code,
    reason = "one immutable value follows Release publication"
)]
// SAFETY: only the winning EMPTY -> WRITING CAS writes the value. Readers
// copy a Copy + Sync payload only after Acquire observes READY. Non-Copy
// owners remain opaque until exclusive destruction or reset. No shared
// operation mutates a published payload. Exclusive reset cannot overlap any
// shared access, and T: Send permits its cross-context ownership transfer.
unsafe impl<T: Send> Sync for OnceValue<T> {}

impl<T> OnceValue<T> {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Whether a writer claimed publication, including while preempted.
    pub fn is_requested(&self) -> bool {
        self.state.load(Ordering::Acquire) != EMPTY
    }

    /// Publish once. A competing or later writer gets its whole value back.
    pub fn set(&self, value: T) -> Result<(), T> {
        self.initialize(|| value).map_err(|make| make())
    }

    fn initialize<F: FnOnce() -> T>(&self, make: F) -> Result<(), F> {
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(make);
        }
        let value = make();
        #[allow(unsafe_code, reason = "the winning writer owns the unpublished cell")]
        // SAFETY: this call alone changed EMPTY to WRITING; no reader accesses
        // the payload before READY and no other writer can enter this epoch.
        unsafe {
            (*self.value.get()).write(value)
        };
        self.state.store(READY, Ordering::Release);
        Ok(())
    }

    /// Copy the published value, or return immediately before it is ready.
    pub fn get(&self) -> Option<T>
    where
        T: Copy + Sync,
    {
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        #[allow(
            unsafe_code,
            reason = "Acquire observed the immutable initialized payload"
        )]
        // SAFETY: READY follows initialization by the unique writer's Release
        // store. T is Copy and no shared operation writes this cell again.
        Some(unsafe { (*self.value.get()).assume_init_read() })
    }

    /// Begin a new epoch only after every old endpoint stopped borrowing it.
    pub fn reset(&mut self) {
        let ready = core::mem::replace(self.state.get_mut(), EMPTY) == READY;
        if ready {
            #[allow(
                unsafe_code,
                reason = "exclusive reset owns the initialized retained value"
            )]
            // SAFETY: READY contains the one initialized value, and exclusive
            // access ends all shared readers/writers. Clear state before Drop,
            // including if T's destructor unwinds.
            unsafe {
                self.value.get_mut().assume_init_drop()
            };
        }
    }
}

impl<T> Default for OnceValue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceValue<T> {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn an_interrupted_writer_blocks_neither_the_reader_nor_a_competing_writer() {
        let value = OnceValue::new();
        assert!(!value.is_requested());
        assert_eq!(value.get(), None);
        assert!(
            value
                .initialize(|| {
                    assert!(value.is_requested());
                    assert_eq!(value.get(), None);
                    assert_eq!(value.set(9), Err(9));
                    7
                })
                .is_ok()
        );
        assert_eq!(value.get(), Some(7));
        assert_eq!(value.set(8), Err(8));
        assert_eq!(value.get(), Some(7));
    }

    #[test]
    fn only_an_exclusive_reset_starts_another_epoch() {
        let mut value = OnceValue::new();
        assert_eq!(value.set(7), Ok(()));
        value.reset();
        assert!(!value.is_requested());
        assert_eq!(value.get(), None);
        assert_eq!(value.set(8), Ok(()));
        assert_eq!(value.get(), Some(8));
    }

    #[test]
    fn a_non_copy_owner_stays_retained_until_exclusive_destruction() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        };
        struct Counted(Arc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, AtomicOrdering::SeqCst);
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let mut retained = OnceValue::new();
        assert!(retained.set(Counted(count.clone())).is_ok());
        assert!(retained.is_requested());
        assert_eq!(count.load(AtomicOrdering::SeqCst), 0);
        retained.reset();
        assert_eq!(count.load(AtomicOrdering::SeqCst), 1);
        assert!(retained.set(Counted(count.clone())).is_ok());
        drop(retained);
        assert_eq!(count.load(AtomicOrdering::SeqCst), 2);
    }
}
