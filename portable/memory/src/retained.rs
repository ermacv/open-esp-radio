//! Records in memory that survives a reset.
//!
//! A reset-retained region keeps what the previous boot wrote, so the boot
//! reads it before anything initialized it: a record there is of a type every
//! bit pattern is a value of ([`bytemuck::AnyBitPattern`]). A
//! [`RetainedCell`] holds one such record and hands it to one holder at a
//! time, without waiting: its claim lives in memory every boot zeroes, so a
//! reset never leaves the record claimed. A holder that finds it claimed, a
//! fatal path on the other hart or a record interrupted on its own, skips
//! its write instead of racing it.
#![allow(
    unsafe_code,
    reason = "reading retained memory as a value needs the any-bits promise"
)]

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use bytemuck::AnyBitPattern;

/// The storage of one [`RetainedCell`], which the cell's static places in the
/// retained region. Only [`retained_static!`](crate::retained_static) creates
/// one.
#[doc(hidden)]
pub struct RetainedStorage<T>(UnsafeCell<T>);

impl<T: AnyBitPattern> RetainedStorage<T> {
    /// Storage with an all-zero initializer, which a NOLOAD section never
    /// loads: the record keeps what the previous boot left.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        // SAFETY: an `AnyBitPattern` type is `Zeroable`: all zero bytes are
        // one of its values.
        Self(UnsafeCell::new(unsafe { core::mem::zeroed() }))
    }
}

// SAFETY: the value is reached only through the one cell's claim, which
// admits one holder at a time; the holder may be on any hart.
unsafe impl<T: Send> Sync for RetainedStorage<T> {}

/// One record in reset-retained memory, handed to one holder at a time.
pub struct RetainedCell<T: 'static> {
    storage: &'static RetainedStorage<T>,
    held: &'static AtomicBool,
}

impl<T: AnyBitPattern> RetainedCell<T> {
    /// # Safety
    ///
    /// No other cell refers to `storage` or `held`, and `held` is false at
    /// every boot: [`retained_static!`](crate::retained_static) defines both
    /// for its cell alone, `held` in zeroed memory.
    #[doc(hidden)]
    pub const unsafe fn __new(
        storage: &'static RetainedStorage<T>,
        held: &'static AtomicBool,
    ) -> Self {
        Self { storage, held }
    }

    /// The record, unless another holder has it; never waits. Inlined, so a
    /// fatal path placed in SRAM stays there.
    #[inline(always)]
    pub fn try_lock(&self) -> Option<RetainedGuard<'_, T>> {
        self.held
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(RetainedGuard { cell: self })
    }
}

/// The holder's access to a [`RetainedCell`]'s record; dropping it releases
/// the record.
pub struct RetainedGuard<'a, T: 'static> {
    cell: &'a RetainedCell<T>,
}

impl<T> Deref for RetainedGuard<'_, T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the cell's claim, so no other reference to
        // the value exists while it lives.
        unsafe { &*self.cell.storage.0.get() }
    }
}

impl<T> DerefMut for RetainedGuard<'_, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`, and `&mut self` excludes the guard's own
        // shared borrows.
        unsafe { &mut *self.cell.storage.0.get() }
    }
}

impl<T> Drop for RetainedGuard<'_, T> {
    #[inline(always)]
    fn drop(&mut self) {
        self.cell.held.store(false, Ordering::Release);
    }
}

/// Define a [`RetainedCell`] static whose record lives in `$section`, a
/// NOLOAD reset-retained section, and whose claim lives in `$held`, a
/// section every boot zeroes; the platform's own macro names both, so that a
/// fatal path reaches them without a cache.
#[doc(hidden)]
#[macro_export]
macro_rules! __retained_static {
    ($(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty, in $section:literal, held in $held:literal;) => {
        $(#[$attribute])*
        $vis static $name: $crate::retained::RetainedCell<$ty> = {
            #[unsafe(link_section = $section)]
            static STORAGE: $crate::retained::RetainedStorage<$ty> =
                $crate::retained::RetainedStorage::new();
            #[unsafe(link_section = $held)]
            static HELD: ::core::sync::atomic::AtomicBool =
                ::core::sync::atomic::AtomicBool::new(false);
            // SAFETY: `STORAGE` and `HELD` are this block's own statics, and
            // `HELD`, false at zero, is in a section every boot zeroes.
            unsafe { $crate::retained::RetainedCell::__new(&STORAGE, &HELD) }
        };
    };
}

#[cfg(test)]
mod tests {
    #[derive(Clone, Copy, bytemuck::AnyBitPattern)]
    #[repr(C)]
    struct Record {
        word: u32,
        bytes: [u8; 4],
    }

    crate::__retained_static! {
        static CELL: Record, in ".data.retained_test", held in ".bss.retained_test";
    }
    crate::__retained_static! {
        static RING: Record, in ".data.retained_test", held in ".bss.retained_test";
    }
    crate::__retained_static! {
        static FAULT: Record, in ".data.retained_test", held in ".bss.retained_test";
    }

    #[test]
    fn one_holder_at_a_time_and_the_record_stays() {
        let mut first = CELL.try_lock().unwrap();
        assert!(CELL.try_lock().is_none(), "a second holder waits for none");
        first.word = 7;
        drop(first);
        let second = CELL.try_lock().unwrap();
        assert_eq!((second.word, second.bytes), (7, [0; 4]));
    }

    #[test]
    fn a_held_cell_never_holds_up_another() {
        // A fault has its own cell: a checkpoint holding the ring leaves it
        // free.
        let _ring = RING.try_lock().unwrap();
        let mut fault = FAULT.try_lock().unwrap();
        fault.word = 1;
    }
}
