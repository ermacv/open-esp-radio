//! A `&'static T` published once and read lock-free, and a function bound
//! at link time.
#![allow(
    unsafe_code,
    reason = "an atomic pointer is the lock-free form of a static reference"
)]

use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

/// A `&'static T` that one context publishes and any context, an interrupt
/// handler or the panic path among them, reads without a lock. It is a
/// single atomic pointer, so a static of it can live in the SRAM sections
/// such paths reach without a cache.
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct StaticRef<T: 'static> {
    pointer: AtomicPtr<T>,
}

impl<T: Sync + 'static> StaticRef<T> {
    /// No reference yet.
    pub const fn new() -> Self {
        Self {
            pointer: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Publish `value`; a later `set` replaces it.
    #[inline(always)]
    pub fn set(&self, value: &'static T) {
        self.pointer
            .store(ptr::from_ref(value).cast_mut(), Ordering::Release);
    }

    /// The published reference, if any.
    #[inline(always)]
    pub fn get(&self) -> Option<&'static T> {
        let pointer = self.pointer.load(Ordering::Acquire);
        // SAFETY: `set` is the only writer, and it stores a pointer derived
        // from a `&'static T`; the pointee is valid for the rest of the
        // program and, being `Sync`, may be shared with any context.
        unsafe { pointer.as_ref() }
    }
}

impl<T: Sync + 'static> Default for StaticRef<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::StaticRef;

    static VALUE: u32 = 7;

    #[test]
    fn a_reference_is_none_until_published_then_the_published_one() {
        let slot = StaticRef::<u32>::new();
        assert!(slot.get().is_none());
        slot.set(&VALUE);
        assert!(core::ptr::eq(slot.get().unwrap(), &VALUE));
    }
}
