//! Reusable inline storage whose initialized value is never a return temporary.

use core::{
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr,
};

/// An optional resident owner, constructed directly in its containing object.
/// Moving an empty place does not move a `T`. References exist only after a
/// complete initializer succeeds; clearing drops the resident owner once.
/// Dereferencing an empty place panics; `as_ref` and `as_mut` inspect it safely.
pub struct InlinePlace<T> {
    value: MaybeUninit<T>,
    initialized: bool,
}

impl<T> InlinePlace<T> {
    pub const fn empty() -> Self {
        Self {
            value: MaybeUninit::uninit(),
            initialized: false,
        }
    }

    pub const fn as_ref(&self) -> Option<&T> {
        if !self.initialized {
            return None;
        }
        #[allow(
            unsafe_code,
            reason = "the completed initializer owns this resident value"
        )]
        // SAFETY: initialized is set only after the checked initializer completes.
        Some(unsafe { self.value.assume_init_ref() })
    }

    pub fn as_mut(&mut self) -> Option<&mut T> {
        if !self.initialized {
            return None;
        }
        #[allow(unsafe_code, reason = "the exclusive place owns its initialized value")]
        // SAFETY: initialized proves a complete T; the exclusive loan excludes readers.
        Some(unsafe { self.value.assume_init_mut() })
    }

    pub fn initialize(&mut self, make: impl FnOnce(&mut MaybeUninit<T>) -> &mut T) -> &mut T {
        self.clear();
        let destination = self.value.as_mut_ptr();
        let value = make(&mut self.value);
        assert!(
            ptr::eq(value, destination),
            "initializer must use its destination"
        );
        self.initialized = true;
        value
    }

    pub fn clear(&mut self) {
        if core::mem::replace(&mut self.initialized, false) {
            #[allow(
                unsafe_code,
                reason = "exclusive clearing owns the resident destructor"
            )]
            // SAFETY: the flag proved initialization and was cleared before Drop can unwind.
            unsafe {
                self.value.assume_init_drop()
            };
        }
    }
}

impl<T> Default for InlinePlace<T> {
    fn default() -> Self {
        Self::empty()
    }
}
impl<T> Drop for InlinePlace<T> {
    fn drop(&mut self) {
        self.clear();
    }
}
impl<T> Deref for InlinePlace<T> {
    type Target = T;
    #[expect(
        clippy::expect_used,
        reason = "Deref requires an initialized place; optional access uses as_ref"
    )]
    fn deref(&self) -> &T {
        self.as_ref()
            .expect("resident place must be initialized before access")
    }
}
impl<T> DerefMut for InlinePlace<T> {
    #[expect(
        clippy::expect_used,
        reason = "DerefMut requires an initialized place; optional access uses as_mut"
    )]
    fn deref_mut(&mut self) -> &mut T {
        self.as_mut()
            .expect("resident place must be initialized before access")
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    struct Owner<'a>(&'a Cell<usize>);
    impl Drop for Owner<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn replacement_constructs_at_the_resident_address_and_drops_once() {
        let drops = Cell::new(0);
        let mut place: InlinePlace<Owner<'_>> = InlinePlace::empty();
        assert!(place.as_ref().is_none());
        let first = place.initialize(|p| p.write(Owner(&drops))) as *mut _;
        let second = place.initialize(|p| p.write(Owner(&drops))) as *mut _;
        assert_eq!(first, second);
        assert_eq!(drops.get(), 1);
        place.clear();
        assert!(place.as_mut().is_none());
        drop(place);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn interrupted_replacement_leaves_an_empty_reusable_place() {
        let drops = Cell::new(0);
        let mut place: InlinePlace<Owner<'_>> = InlinePlace::empty();
        place.initialize(|p| p.write(Owner(&drops)));
        let result = catch_unwind(AssertUnwindSafe(|| {
            place.initialize(|_| panic!("interrupted initializer"));
        }));
        assert!(result.is_err());
        assert!(place.as_ref().is_none());
        assert_eq!(drops.get(), 1);
        place.initialize(|p| p.write(Owner(&drops)));
        drop(place);
        assert_eq!(drops.get(), 2);
    }
}
