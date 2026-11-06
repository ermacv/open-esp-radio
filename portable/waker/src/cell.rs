//! The atomics and the cell, from `core` or from the model checker.

#[cfg(feature = "loom")]
pub(crate) use loom::{cell::UnsafeCell, sync::atomic::AtomicUsize};

#[cfg(not(feature = "loom"))]
pub(crate) use core::sync::atomic::AtomicUsize;

/// `core`'s cell under the model checker's access method.
#[cfg(not(feature = "loom"))]
#[derive(Debug)]
pub(crate) struct UnsafeCell<T>(core::cell::UnsafeCell<T>);

#[cfg(not(feature = "loom"))]
impl<T> UnsafeCell<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(core::cell::UnsafeCell::new(value))
    }

    pub(crate) fn with_mut<R>(&self, access: impl FnOnce(*mut T) -> R) -> R {
        access(self.0.get())
    }
}
