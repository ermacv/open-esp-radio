//! Statics in regions the boot zeroes.
//!
//! A board's linker script places some statics, by section name, in regions
//! the boot clears instead of loading (`NOLOAD`): their initializer never
//! reaches the device. Such a static must therefore start as all zero bytes,
//! and those bytes must be a valid value that means what the constructor
//! means. [`Zeroable`] (bytemuck's marker, the one esp-hal's
//! `#[ram(zeroed)]` requires) marks the types for which that holds;
//! `#[derive(bytemuck::Zeroable)]` accepts a struct whose fields all are, or
//! an enum with an integer `repr` and a zero discriminant. [`zeroed`] is the
//! one initializer [`zeroed_static!`](crate::zeroed_static) gives such a
//! static. The image linker checks the result: a zeroed input section that
//! carries an initializer fails the link.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

pub use bytemuck::Zeroable;

/// The all-zero value of `T`, its initial state; usable in a `static`
/// initializer, where `Zeroable::zeroed` is not.
#[allow(
    unsafe_code,
    reason = "Zeroable guarantees the zero bit pattern is valid"
)]
pub const fn zeroed<T: Zeroable>() -> T {
    // SAFETY: `T: Zeroable` guarantees that all-zero bytes are a valid `T`.
    unsafe { core::mem::zeroed() }
}

/// A static, in a region the boot zeroes, that one owner claims once.
///
/// It starts as [`zeroed`]; [`Self::take`] hands out the value the first
/// time. A value set at runtime is a `ZeroedStatic<MaybeUninit<T>>` that the
/// claimant writes.
pub struct ZeroedStatic<T> {
    taken: AtomicBool,
    value: UnsafeCell<T>,
}

#[allow(unsafe_code, reason = "the cell hands its value to one claimant")]
// SAFETY: the cell offers no access but `take`, which gives the value to one
// caller only (`taken` swaps once). Only that caller's thread ever touches
// it: the zero value was made by no thread and a runtime value is written
// by the claimant; handing the `&'static mut T` on needs `T: Send` by the
// ordinary rules. `static_cell::StaticCell` rests on the same argument.
unsafe impl<T> Sync for ZeroedStatic<T> {}

#[allow(unsafe_code, reason = "a flag and a zero-valid value")]
// SAFETY: `taken` is false at zero and `value` is zero-valid.
unsafe impl<T: Zeroable> Zeroable for ZeroedStatic<T> {}

impl<T: Zeroable> ZeroedStatic<T> {
    /// The unclaimed cell with the zero value.
    pub const fn new() -> Self {
        zeroed()
    }
}

impl<T: Zeroable> Default for ZeroedStatic<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ZeroedStatic<T> {
    /// The value, the first time; panics on a second claim.
    pub fn take(&'static self) -> &'static mut T {
        self.try_take().expect("a zeroed static is claimed once")
    }

    /// The value, the first time; `None` once claimed.
    #[allow(unsafe_code, reason = "the first claimant receives the only reference")]
    #[allow(
        clippy::mut_from_ref,
        reason = "a take-once cell: the swap of `taken` makes this the only reference"
    )]
    pub fn try_take(&'static self) -> Option<&'static mut T> {
        if self.taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        // SAFETY: the swap above returned false exactly once, so no other
        // reference to `value` exists or will be created.
        Some(unsafe { &mut *self.value.get() })
    }
}

impl<T> ZeroedStatic<MaybeUninit<T>> {
    /// Claim the storage once and move `value` into it; panics on a second
    /// claim.
    pub fn init(&'static self, value: T) -> &'static mut T {
        self.take().write(value)
    }
}

const ONCE_EMPTY: u8 = 0;
const ONCE_WRITING: u8 = 1;
const ONCE_READY: u8 = 2;

/// A shared value in a region the boot zeroes, written once at runtime.
///
/// Zero bytes are the empty cell, whatever `T`'s own layout: the value is
/// `MaybeUninit` until [`Self::get_or_init`] writes it, so a type without a
/// zero representation (a waker, a mutex) can live in a zeroed static that an
/// interrupt reads with [`Self::get`].
pub struct ZeroedOnce<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

#[allow(unsafe_code, reason = "the value is shared only once written")]
// SAFETY: readers get `&T` only after the `ONCE_READY` publication, so the
// cell shares a `T` across threads (`T: Sync`) that one thread wrote and
// others may drop never (`T: Send` for the writer's hand-over).
unsafe impl<T: Send + Sync> Sync for ZeroedOnce<T> {}

#[allow(unsafe_code, reason = "a zero state and uninitialized storage")]
// SAFETY: state 0 is `ONCE_EMPTY` and `MaybeUninit` accepts any bytes.
unsafe impl<T> Zeroable for ZeroedOnce<T> {}

impl<T> ZeroedOnce<T> {
    /// The empty cell: all zero bytes.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(ONCE_EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// The value, once written; never waits, so an interrupt may call it.
    #[inline(always)]
    #[allow(unsafe_code, reason = "the value is read only after its publication")]
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) != ONCE_READY {
            return None;
        }
        // SAFETY: `ONCE_READY` is stored with Release only after the value
        // was written, and the value is never written again.
        Some(unsafe { (*self.value.get()).assume_init_ref() })
    }

    /// The value, writing `init()` first if no one has. The cell has one
    /// writing context at a time (an interrupt reads with [`Self::get`]); a
    /// call that finds another write in progress panics rather than wait.
    #[allow(unsafe_code, reason = "the winning exchange is the only writer")]
    pub fn get_or_init(&self, init: impl FnOnce() -> T) -> &T {
        match self.state.compare_exchange(
            ONCE_EMPTY,
            ONCE_WRITING,
            Ordering::Acquire,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // SAFETY: the exchange to `ONCE_WRITING` succeeded once; no
                // reader sees the value before `ONCE_READY`.
                unsafe { (*self.value.get()).write(init()) };
                self.state.store(ONCE_READY, Ordering::Release);
            }
            Err(state) => assert_eq!(
                state, ONCE_READY,
                "a zeroed once cell is written by one context at a time"
            ),
        }
        self.get().expect("a written once cell is ready")
    }
}

impl<T> Default for ZeroedOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for ZeroedOnce<T> {
    #[allow(unsafe_code, reason = "a ready cell owns its value")]
    fn drop(&mut self) {
        if *self.state.get_mut() == ONCE_READY {
            // SAFETY: `ONCE_READY` means the value was written; `&mut self`
            // excludes every reader.
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}

/// Declare statics in a region the boot zeroes: each starts as
/// [`zeroed`](crate::zeroed::zeroed), so its type must be
/// [`Zeroable`](crate::zeroed::Zeroable) and no non-zero initializer can be
/// written. The macro names the section, so the caller writes no `unsafe`
/// attribute; `static mut` is accepted for storage handed to hardware or
/// assembly by address.
///
/// ```ignore
/// oer_memory::zeroed_static! {
///     /// Placement: packet payloads live in the PSRAM tier.
///     static POOL: ZeroedStatic<Storage> = zeroed in ".psram.bss.pool";
/// }
/// ```
#[macro_export]
macro_rules! zeroed_static {
    () => {};
    (
        $(#[$meta:meta])*
        $vis:vis static mut $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[unsafe(link_section = $section)]
        $vis static mut $name: $ty = $crate::zeroed::zeroed();
        $crate::zeroed_static! { $($rest)* }
    };
    (
        $(#[$meta:meta])*
        $vis:vis static $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[unsafe(link_section = $section)]
        $vis static $name: $ty = $crate::zeroed::zeroed();
        $crate::zeroed_static! { $($rest)* }
    };
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicU32;

    use super::*;

    #[test]
    fn a_zeroed_once_cell_is_empty_until_written_once() {
        static CELL: ZeroedOnce<[u8; 4]> = zeroed();
        assert!(CELL.get().is_none());
        assert_eq!(CELL.get_or_init(|| [1, 2, 3, 4]), &[1, 2, 3, 4]);
        // A second initializer does not run.
        assert_eq!(CELL.get_or_init(|| unreachable!()), &[1, 2, 3, 4]);
        assert_eq!(CELL.get(), Some(&[1, 2, 3, 4]));
    }

    #[test]
    fn dropping_a_written_once_cell_drops_its_value() {
        extern crate std;
        use std::rc::Rc;
        let shared = Rc::new(());
        {
            let cell = ZeroedOnce::new();
            cell.get_or_init(|| Rc::clone(&shared));
            assert_eq!(Rc::strong_count(&shared), 2);
        }
        assert_eq!(Rc::strong_count(&shared), 1);
    }

    #[test]
    fn a_zeroed_static_starts_unclaimed_and_hands_its_zero_value_once() {
        static CELL: ZeroedStatic<[AtomicU32; 4]> = ZeroedStatic::new();
        let value = CELL.take();
        assert!(value.iter().all(|word| word.load(Ordering::Relaxed) == 0));
        assert!(CELL.try_take().is_none());
    }

    #[test]
    fn a_runtime_value_is_written_into_uninitialized_zeroed_storage() {
        static CELL: ZeroedStatic<MaybeUninit<[u8; 3]>> = ZeroedStatic::new();
        let value = CELL.take().write([1, 2, 3]);
        assert_eq!(*value, [1, 2, 3]);
    }

    #[test]
    fn the_zero_initializer_is_all_zero_bytes() {
        let cell: ZeroedStatic<[u8; 16]> = ZeroedStatic::new();
        assert!(!cell.taken.load(Ordering::Relaxed));
        assert_eq!(cell.value.into_inner(), [0; 16]);
    }
}
