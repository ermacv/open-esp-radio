//! Storage accessed only in its owning execution context.

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
};

/// Proof that the caller may access a cell in its one execution context.
///
/// # Safety
/// Implementations must keep proof construction and use private to the owner:
/// for each cell reached with a proof, every access must run in the same
/// context, never concurrently with itself, including across preemption and
/// across cores. A proof must not escape that context. Initialization and
/// destruction must also respect any thread restrictions of the payload.
/// Separate cells may belong to separate contexts even with the same proof
/// type; the implementation must prevent passing a proof to another context's
/// cell. This is a memory-safety promise, not a scheduling or lifecycle rule.
#[allow(unsafe_code, reason = "the owner proves serialization of each cell")]
// SAFETY: implementors establish the per-cell execution and payload-transfer
// contract above; the cell relies on it for its non-atomic borrow flag.
pub unsafe trait ContextProof {}

/// One value lent only in the execution context its private proof names.
///
/// The owner keeps both the proof and access to the cell private, admitting
/// access only after checking the owning context. The cell does not check a
/// hart or executor itself and never waits or masks interrupts.
///
/// [`with`](Self::with) refuses a nested borrow of this same cell with `None`,
/// without calling the closure or changing the payload. Borrowing a different
/// cell is allowed. The borrow ends even when the closure unwinds.
/// The cell's `Send` auto trait retains the payload's and proof's move restrictions.
pub struct ContextCell<T, P: ContextProof> {
    value: UnsafeCell<T>,
    borrowed: Cell<bool>,
    proof: PhantomData<P>,
}

#[allow(
    unsafe_code,
    reason = "the private proof confines access to one context"
)]
// SAFETY: P confines every access to this cell to one non-concurrent context
// and respects T's thread restrictions. The borrow flag refuses nested calls
// before they can create another mutable reference to the payload.
unsafe impl<T, P: ContextProof> Sync for ContextCell<T, P> {}

impl<T, P: ContextProof> ContextCell<T, P> {
    /// Store a value for the execution context proved by `P`.
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            borrowed: Cell::new(false),
            proof: PhantomData,
        }
    }

    /// Lend the value in its owning context, or refuse a nested call.
    #[inline(always)]
    pub fn with<R>(&self, _proof: &P, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if self.borrowed.replace(true) {
            return None;
        }
        let _borrow = Borrow(&self.borrowed);
        #[allow(unsafe_code, reason = "the proof and borrow flag admit one reference")]
        // SAFETY: P proves the owning context and excludes concurrent access.
        // The flag was clear, so no enclosing call retains a payload borrow;
        // Borrow clears it only after f's exclusive borrow ends, also on unwind.
        let value = unsafe { &mut *self.value.get() };
        Some(f(value))
    }
}

struct Borrow<'a>(&'a Cell<bool>);
impl Drop for Borrow<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    reason = "tests exercise refused callbacks and unwinding"
)]
mod tests {
    extern crate std;
    use super::*;

    struct Local(PhantomData<*mut ()>);
    #[allow(
        unsafe_code,
        reason = "each test keeps its proof and cells on its own thread"
    )]
    // SAFETY: Local is private and cannot leave its thread; each test accesses
    // only its own cells, initialized and destroyed on that same thread.
    unsafe impl ContextProof for Local {}

    #[test]
    fn ordinary_access_retains_changes_and_returns_the_closure_result() {
        let proof = Local(PhantomData);
        let cell = ContextCell::new(3);
        assert_eq!(
            cell.with(&proof, |value| {
                *value += 4;
                *value
            }),
            Some(7)
        );
        assert_eq!(cell.with(&proof, |value| *value), Some(7));
    }

    #[test]
    fn nesting_refuses_the_same_cell_without_ending_the_outer_borrow() {
        let proof = Local(PhantomData);
        let cell = ContextCell::new(1);
        let other = ContextCell::new(2);
        assert_eq!(
            cell.with(&proof, |value| {
                assert_eq!(
                    cell.with(&proof, |_| panic!("nested closure must not run")),
                    None::<()>
                );
                assert_eq!(other.with(&proof, |other| *other), Some(2));
                assert_eq!(cell.with(&proof, |_| ()), None);
                *value += 1;
            }),
            Some(())
        );
        assert_eq!(cell.with(&proof, |value| *value), Some(2));
    }

    #[test]
    fn unwinding_ends_the_borrow() {
        let proof = Local(PhantomData);
        let cell = ContextCell::new(1);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cell.with(&proof, |value| {
                    *value = 2;
                    panic!("test unwind");
                });
            }))
            .is_err()
        );
        assert_eq!(cell.with(&proof, |value| *value), Some(2));
    }
}
