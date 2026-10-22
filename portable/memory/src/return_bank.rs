//! Affine return of values which may move between execution contexts.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};

const EMPTY: u8 = 0;
const AVAILABLE: u8 = 1;
const OUT: u8 = 2;

struct ReturnCell<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

#[allow(
    unsafe_code,
    reason = "the available cell or its affine return token owns each value"
)]
// SAFETY: take's successful Acquire CAS grants the sole payload read. Its
// unique token grants the sole write, followed by Release publication. No
// other reader or writer accesses a cell while that token is outstanding.
unsafe impl<T: Send> Sync for ReturnCell<T> {}

/// A fixed bank with one affine return right per outstanding value.
///
/// Returning a value consumes its token; no producer endpoint is shared.
/// A lost token leaves its cell unavailable permanently. Initialization needs
/// exclusive access; shared operations never wait for another context.
pub struct ReturnBank<T, const N: usize> {
    cells: [ReturnCell<T>; N],
}

impl<T, const N: usize> ReturnBank<T, N> {
    pub const fn new() -> Self {
        Self {
            cells: [const {
                ReturnCell {
                    state: AtomicU8::new(EMPTY),
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                }
            }; N],
        }
    }

    /// Seed one previously empty place before publishing any endpoints.
    pub fn seed(&mut self, index: usize, value: T) -> Result<(), T> {
        let Some(cell) = self.cells.get_mut(index) else {
            return Err(value);
        };
        if *cell.state.get_mut() != EMPTY {
            return Err(value);
        }
        cell.value.get_mut().write(value);
        *cell.state.get_mut() = AVAILABLE;
        Ok(())
    }

    /// Take one complete value and its sole right of return.
    pub fn take(&self, index: usize) -> Option<(T, ReturnToken<'_, T>)> {
        let cell = self.cells.get(index)?;
        cell.state
            .compare_exchange(AVAILABLE, OUT, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        #[allow(
            unsafe_code,
            reason = "the successful CAS transfers the initialized value"
        )]
        // SAFETY: AVAILABLE follows seed or a token's Release publication.
        // This call alone changed it to OUT, and only its new token can write
        // the cell again after this complete read.
        let value = unsafe { (*cell.value.get()).assume_init_read() };
        Some((value, ReturnToken { cell }))
    }

    /// Whether all seeded places are available, with no outstanding token.
    pub fn all_returned(&self) -> bool {
        self.cells
            .iter()
            .all(|cell| cell.state.load(Ordering::Acquire) == AVAILABLE)
    }
}

impl<T, const N: usize> Default for ReturnBank<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for ReturnCell<T> {
    fn drop(&mut self) {
        if *self.state.get_mut() == AVAILABLE {
            #[allow(
                unsafe_code,
                reason = "exclusive destruction owns the still-available value"
            )]
            // SAFETY: AVAILABLE contains an initialized value. Exclusive
            // access proves no token or concurrent take can still borrow it.
            unsafe {
                self.value.get_mut().assume_init_drop()
            };
        }
    }
}

/// The sole right to return a value to one bank place.
///
/// This token may move with its value to another context. Dropping or
/// forgetting it never makes the place available and touches no payload.
#[must_use = "retain with the value or consume to return it"]
pub struct ReturnToken<'a, T> {
    cell: &'a ReturnCell<T>,
}

impl<T> ReturnToken<'_, T> {
    pub fn return_value(self, value: T) {
        #[allow(unsafe_code, reason = "the unique token owns this unavailable cell")]
        // SAFETY: take created exactly one token after moving the old value
        // out. Its cell stays OUT until this consuming write publishes the
        // replacement. No other operation can access this payload meanwhile.
        unsafe {
            (*self.cell.value.get()).write(value)
        };
        self.cell.state.store(AVAILABLE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn a_returned_place_can_be_taken_once_in_each_epoch() {
        let mut bank = ReturnBank::<_, 1>::new();
        assert!(!bank.all_returned());
        assert!(bank.seed(0, 7).is_ok());
        assert_eq!(bank.seed(0, 8), Err(8));
        let (value, token) = bank.take(0).expect("seeded");
        assert_eq!(value, 7);
        assert!(!bank.all_returned());
        assert!(bank.take(0).is_none());
        token.return_value(9);
        assert!(bank.all_returned());
        let (value, token) = bank.take(0).expect("returned");
        assert_eq!(value, 9);
        token.return_value(value);
    }

    #[test]
    fn losing_a_return_right_never_reissues_the_place() {
        let mut bank = ReturnBank::<_, 2>::new();
        assert!(bank.seed(0, 7).is_ok());
        assert!(bank.seed(1, 8).is_ok());
        {
            let (_, _lost) = bank.take(0).expect("seeded");
        }
        assert!(!bank.all_returned());
        assert!(bank.take(0).is_none());
        assert!(bank.take(1).is_some());
    }

    #[test]
    fn the_return_right_moves_between_contexts_without_sharing_a_producer() {
        let mut bank = ReturnBank::<_, 1>::new();
        assert!(bank.seed(0, 7).is_ok());
        let (value, token) = bank.take(0).expect("seeded");
        std::thread::scope(|scope| {
            scope.spawn(move || token.return_value(value + 1));
        });
        let (value, token) = bank.take(0).expect("cross-context return");
        assert_eq!(value, 8);
        token.return_value(value);
        assert!(bank.all_returned());
    }
}
