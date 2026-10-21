//! First-writer publication without waiting for an interrupted writer.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

/// One immutable value published by the first writer.
///
/// A higher-priority reader never waits for a preempted writer: `get` returns
/// `None` until publication completes, while `is_requested` already reports
/// the request. The caller supplies its own wake after `set`. Reset requires
/// exclusive access, after every endpoint of the previous epoch ended.
pub struct OnceValue<T: Copy> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

#[allow(
    unsafe_code,
    reason = "one immutable value follows Release publication"
)]
// SAFETY: only the winning EMPTY -> WRITING CAS writes the value. Readers
// access it only after Acquire observes READY, and no shared operation can
// mutate it again. Exclusive reset cannot overlap any shared access.
unsafe impl<T: Copy + Send + Sync> Sync for OnceValue<T> {}

impl<T: Copy> OnceValue<T> {
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
    pub fn get(&self) -> Option<T> {
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
        *self.state.get_mut() = EMPTY;
    }
}

impl<T: Copy> Default for OnceValue<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
