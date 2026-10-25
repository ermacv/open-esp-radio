//! One value its holders take turns with, without waiting, until one keeps
//! it for good.
//!
//! A [`ClaimCell`] holds a value installed once ([`ClaimCell::install`]).
//! While it is free, a caller borrows it for one call ([`ClaimCell::lend`]),
//! moves it out for good ([`ClaimCell::take`]) or keeps it for good in the
//! cell and borrows it for one last call ([`ClaimCell::seal`]). Each is one
//! compare-and-swap of the cell's state: a caller that does not find the
//! cell free (a lend on another hart, or one it interrupted) is refused at
//! once and never waits, so an interrupt handler or a fatal path may call it.
//! A taken or sealed cell never lends again.
#![allow(
    unsafe_code,
    reason = "the cell lends or moves its value under one winning exchange"
)]

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};

#[cfg(test)]
mod tests;

/// No value yet; the zero state.
const EMPTY: u8 = 0;
/// An install is storing the value.
const INSTALLING: u8 = 1;
/// The value is installed and no one holds it.
const FREE: u8 = 2;
/// The value is lent to one [`ClaimCell::lend`] call.
const LENT: u8 = 3;
/// The value was moved out by [`ClaimCell::take`], for good.
const TAKEN: u8 = 4;
/// The value stays in the cell for the one [`ClaimCell::seal`] call, for
/// good.
const SEALED: u8 = 5;

/// Why a cell refused a caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Claim {
    /// Nothing was installed yet.
    Empty,
    /// One lend holds the value now: it is free again when that call ends.
    Lent,
    /// The value was taken or sealed for good.
    Kept,
}

/// One value, installed once, lent to one call at a time until it is taken
/// out or sealed for good.
///
/// The value may reach another context than the one that installed it:
///
/// ```compile_fail,E0277
/// extern crate std;
/// use oer_memory::ClaimCell;
/// // `Rc` cannot leave the context that made it.
/// static CELL: ClaimCell<std::rc::Rc<()>> = ClaimCell::new();
/// ```
pub struct ClaimCell<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: every reference to the value comes from one exchange of `state`
// that admits a single caller (INSTALLING, LENT, TAKEN or SEALED), on any
// core, and the value moves between contexts only as `T: Send` permits.
unsafe impl<T: Send> Sync for ClaimCell<T> {}

impl<T> ClaimCell<T> {
    /// The empty cell.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Move `value` into the cell; it is free from now on.
    ///
    /// # Errors
    ///
    /// The cell had a value (or one is being installed): `value` comes
    /// back unchanged.
    pub fn install(&self, value: T) -> Result<(), T> {
        if self
            .state
            .compare_exchange(EMPTY, INSTALLING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }
        // SAFETY: the exchange to INSTALLING succeeded for this call alone,
        // and no other exchange admits a caller before FREE is stored below,
        // so no reference to the slot exists.
        unsafe { (*self.value.get()).write(value) };
        // Publishes the value to the exchange that takes FREE with Acquire.
        self.state.store(FREE, Ordering::Release);
        Ok(())
    }

    /// Run `f` on the value, which is free again when `f` returns. Never
    /// waits.
    ///
    /// When `f` unwinds, the value stays lent: later callers are refused.
    ///
    /// # Errors
    ///
    /// The cell is empty, lent now, or taken or sealed for good.
    #[inline(always)]
    pub fn lend<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, Claim> {
        self.claim(LENT)?;
        // SAFETY: the exchange from FREE to LENT succeeded for this call
        // alone; FREE was stored (Release) only after the value was written,
        // and no other exchange admits a caller until LENT returns to FREE
        // below, after the borrow of `f` ended.
        let result = f(unsafe { (*self.value.get()).assume_init_mut() });
        self.state.store(FREE, Ordering::Release);
        Ok(result)
    }

    /// Move the value out for good; the cell never lends again. Never
    /// waits.
    ///
    /// # Errors
    ///
    /// The cell is empty, lent now, or taken or sealed before.
    pub fn take(&self) -> Result<T, Claim> {
        self.claim(TAKEN)?;
        // SAFETY: the exchange from FREE to TAKEN succeeded for this call
        // alone and no exchange leaves TAKEN, so the initialized value is
        // read out once and never reached again, dropped or not.
        Ok(unsafe { (*self.value.get()).assume_init_read() })
    }

    /// Keep the value in the cell for good and run `f` on it, once; the
    /// cell never lends again. Never waits.
    ///
    /// # Errors
    ///
    /// The cell is empty, lent now, or taken or sealed before.
    #[inline(always)]
    pub fn seal<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, Claim> {
        self.claim(SEALED)?;
        // SAFETY: the exchange from FREE to SEALED succeeded for this call
        // alone and no exchange leaves SEALED, so this is the last and only
        // reference to the initialized value.
        Ok(f(unsafe { (*self.value.get()).assume_init_mut() }))
    }

    /// Move the cell from FREE to `next`, or say why not.
    #[inline(always)]
    fn claim(&self, next: u8) -> Result<(), Claim> {
        match self
            .state
            .compare_exchange(FREE, next, Ordering::Acquire, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(EMPTY | INSTALLING) => Err(Claim::Empty),
            Err(LENT) => Err(Claim::Lent),
            Err(_) => Err(Claim::Kept),
        }
    }
}

impl<T> Default for ClaimCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for ClaimCell<T> {
    fn drop(&mut self) {
        if matches!(*self.state.get_mut(), FREE | SEALED) {
            // SAFETY: FREE and SEALED hold the initialized value, which no
            // reference reaches once the cell itself is dropped.
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}
