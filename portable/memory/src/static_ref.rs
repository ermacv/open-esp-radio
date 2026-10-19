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

// SAFETY: all zero bytes are the null pointer `new` stores.
unsafe impl<T> bytemuck::Zeroable for StaticRef<T> {}

/// A function bound at link time: a crate declares the symbol it calls and
/// the image defines it once, so every call is a direct call that the stack
/// analysis follows, where a function pointer would leave the callee
/// unknown. The declaring crate's own macro writes both halves, so the two
/// signatures are one.
///
/// ```ignore
/// // The calling crate: the declaration, and the macro the image calls.
/// oer_memory::link_fn! { declare fn __oer_clock() -> u64; }
/// macro_rules! clock {
///     ($now:path) => {
///         oer_memory::link_fn! { define fn __oer_clock() -> u64 = $now; }
///     };
/// }
/// ```
#[macro_export]
macro_rules! link_fn {
    (declare $(#[$attribute:meta])* fn $symbol:ident($($argument:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?;) => {
        // SAFETY: the declaring crate's `define` arm, the only definition,
        // has this signature and no precondition, so a call is safe.
        unsafe extern "Rust" {
            $(#[$attribute])*
            safe fn $symbol($($argument: $ty),*) $(-> $ret)?;
        }
    };
    (define fn $symbol:ident($($argument:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? = $definition:path;) => {
        const _: () = {
            #[unsafe(export_name = stringify!($symbol))]
            fn definition($($argument: $ty),*) $(-> $ret)? {
                $definition($($argument),*)
            }
        };
    };
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

    fn seven() -> u32 {
        7
    }
    crate::link_fn! { declare fn __oer_memory_link_test(add: u32) -> u32; }
    fn plus_seven(add: u32) -> u32 {
        seven() + add
    }
    crate::link_fn! { define fn __oer_memory_link_test(add: u32) -> u32 = plus_seven; }

    #[test]
    fn a_linked_function_calls_its_definition() {
        assert_eq!(__oer_memory_link_test(1), 8);
    }
}
