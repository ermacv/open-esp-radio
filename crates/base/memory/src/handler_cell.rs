//! One value an interrupt handler owns.
//!
//! A driver's interrupt line keeps its state (register owners, publishers,
//! counters) in a static the boot zeroes. The task that brings the driver up
//! moves that state there once, through [`HandlerCell::install`]; from then
//! on only the handler reaches it, through [`HandlerCell::with`]. The cell is
//! an ownership state machine, not a lock: a call that does not find the
//! value ready (not installed yet, or already lent to an enclosing call) gets
//! `None` at once and never waits, so a handler cannot deadlock on it. The
//! value is never reclaimed: an installed cell keeps it for the rest of the
//! image's run.

use core::{
    marker::PhantomData,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU8, Ordering},
};

use crate::zeroed::{Zeroable, zeroed};

/// No value; the zero state.
const EMPTY: u8 = 0;
/// An install is storing the value.
const INSTALLING: u8 = 1;
/// The value is installed and not lent.
const READY: u8 = 2;
/// The value is lent to one [`HandlerCell::with`] call.
const BUSY: u8 = 3;

/// One `&'static mut T`, installed once and lent to one call at a time.
///
/// The handler runs in another context than the installer, so the value
/// must be `Send`:
///
/// ```compile_fail,E0277
/// extern crate std;
/// use oer_memory::HandlerCell;
/// // `Rc` cannot leave the context that made it.
/// static CELL: HandlerCell<std::rc::Rc<()>> = HandlerCell::new();
/// ```
pub struct HandlerCell<T> {
    state: AtomicU8,
    value: AtomicPtr<T>,
    /// The cell hands the value to another context: neither `Send` nor
    /// `Sync` follows from the pointer alone.
    _owns: PhantomData<*mut T>,
}

#[allow(unsafe_code, reason = "the cell lends its value to one call at a time")]
// SAFETY: the only access to the value is `with`, whose READY -> BUSY
// exchange admits one call at a time on any core; the installing context
// hands the value to whichever context calls `with`, which `T: Send` permits.
unsafe impl<T: Send> Sync for HandlerCell<T> {}

#[allow(unsafe_code, reason = "the cell carries a reference to its value")]
// SAFETY: moving the cell moves the access to a `&'static mut T` with it,
// which `T: Send` permits.
unsafe impl<T: Send> Send for HandlerCell<T> {}

#[allow(unsafe_code, reason = "an empty state and a null pointer")]
// SAFETY: state 0 is `EMPTY`, in which the null pointer is never read, and
// zero bytes are the null `AtomicPtr`.
unsafe impl<T> Zeroable for HandlerCell<T> {}

impl<T> HandlerCell<T> {
    /// The empty cell: all zero bytes.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: AtomicPtr::new(ptr::null_mut()),
            _owns: PhantomData,
        }
    }

    /// Move `value` into the cell; it then belongs to the cell's handler.
    ///
    /// # Errors
    ///
    /// The cell already has a value (or one is being installed): `value`
    /// comes back unchanged and the installed one stays.
    pub fn install(
        &'static self,
        value: &'static mut T,
    ) -> Result<Installed<'static, T>, &'static mut T> {
        if self
            .state
            .compare_exchange(EMPTY, INSTALLING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }
        self.value.store(value, Ordering::Relaxed);
        // Publishes the pointer and the value's bytes to the `with` that
        // takes READY with Acquire.
        self.state.store(READY, Ordering::Release);
        Ok(Installed { cell: self })
    }

    /// Run `f` on the value; `None` when the cell is empty or the value is
    /// lent to an enclosing call. Never waits, so a handler may call it.
    ///
    /// When `f` unwinds, the value stays lent: later calls get `None`.
    #[inline(always)]
    #[allow(unsafe_code, reason = "the winning exchange lends the only reference")]
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if self
            .state
            .compare_exchange(READY, BUSY, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let value = self.value.load(Ordering::Relaxed);
        // SAFETY: READY was stored (Release) only after `install` stored a
        // pointer from a `&'static mut T` it consumed, so `value` is valid
        // for the rest of the program and no other reference to it exists
        // outside the cell. The exchange to BUSY succeeded for this call
        // alone, and no other call creates a reference until BUSY returns to
        // READY below, after the borrow of `f` ended.
        let result = f(unsafe { &mut *value });
        self.state.store(READY, Ordering::Release);
        Some(result)
    }
}

impl<T> Default for HandlerCell<T> {
    fn default() -> Self {
        zeroed()
    }
}

/// The receipt of a [`HandlerCell::install`]: the cell has its value.
#[must_use = "the receipt is the proof that the handler's value is installed"]
pub struct Installed<'cell, T> {
    cell: &'cell HandlerCell<T>,
}

impl<'cell, T> Installed<'cell, T> {
    /// The installed cell.
    pub fn cell(&self) -> &'cell HandlerCell<T> {
        self.cell
    }
}

#[cfg(test)]
mod tests;
