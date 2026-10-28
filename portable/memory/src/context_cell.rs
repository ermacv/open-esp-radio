//! Storage accessed only in its owning execution context.

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
};

/// Proof that the caller may access a cell in its one execution context.
///
/// # Safety
/// Implementations must keep unchecked proof construction and use private to
/// the owner, or check the execution context on every access:
/// for each cell reached with a proof, every access must run in the same
/// context, never concurrently with itself, including across preemption and
/// across cores. An unchecked proof must not escape that context; a checked
/// proof must refuse access after leaving it. Initialization and
/// destruction must also respect any thread restrictions of the payload.
/// Separate cells may belong to separate contexts even with the same proof
/// type; the implementation must prevent passing a proof to another context's
/// cell. When `Sharing: Sync`, the context check must exclude concurrent
/// access on every thread/core; otherwise the cell is neither shareable nor
/// transferable and its constructing thread is its context. A shared cell
/// requires `T: Send` so initialization/destruction may cross contexts safely.
/// This is a memory-safety promise, not a scheduling or lifecycle rule.
#[allow(unsafe_code, reason = "the owner proves serialization of each cell")]
// SAFETY: implementors establish the per-cell execution and payload-transfer
// contract above; the cell relies on it for its non-atomic borrow flag.
pub unsafe trait ContextProof {
    /// A `Sync` marker permits sharing cells; a non-`Sync` marker confines
    /// them to the constructing thread. Proofs themselves need not be `Sync`.
    type Sharing;

    /// Whether this proof still admits access in the current context.
    fn is_current(&self) -> bool;
}

/// One value lent only in the execution context its private proof names.
///
/// The owner either keeps its unchecked proof private or uses checked
/// [`crate::Entered`] entry. The cell rechecks the proof before touching its
/// borrow flag; it never waits or masks interrupts. Shared cells require a
/// `Sync` sharing marker and a `Send` payload.
///
/// [`with`](Self::with) refuses a nested borrow of this same cell with `None`,
/// without calling the closure or changing the payload. Borrowing a different
/// cell is allowed. The borrow ends even when the closure unwinds.
/// The cell's `Send` auto trait retains the payload's and proof's move restrictions.
pub struct ContextCell<T, P: ContextProof> {
    value: UnsafeCell<T>,
    borrowed: Cell<bool>,
    proof: PhantomData<(P, P::Sharing)>,
}

#[allow(
    unsafe_code,
    reason = "the private proof confines access to one context"
)]
// SAFETY: P confines every access to this cell to one non-concurrent context
// and T: Send respects payload transfer restrictions. The borrow flag
// refuses nested calls before creating another mutable payload reference.
unsafe impl<T: Send, P: ContextProof> Sync for ContextCell<T, P> where P::Sharing: Sync {}

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
    pub fn with<R>(&self, proof: &P, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if !proof.is_current() || self.borrowed.replace(true) {
            return None;
        }
        let _borrow = Borrow(&self.borrowed);
        #[allow(unsafe_code, reason = "the proof and borrow flag admit one reference")]
        // SAFETY: P proves the owning context and excludes concurrent access.
        // The proof was rechecked before the flag, which was clear: no enclosing
        // call retains a payload borrow;
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

    struct Local;
    std::thread_local! { static RUNNING: Cell<bool> = const { Cell::new(true) }; }
    #[allow(
        unsafe_code,
        reason = "each test keeps its proof and cells on its own thread"
    )]
    // SAFETY: Sharing keeps cells on their constructing test thread; RUNNING
    // admits only that thread, and nested access is refused by the borrow flag.
    unsafe impl crate::ExecutionContext for Local {
        type Sharing = *mut ();
        fn running() -> bool {
            RUNNING.with(Cell::get)
        }
    }

    #[test]
    fn ordinary_access_retains_changes_and_returns_the_closure_result() {
        let proof = crate::Entered::<Local>::enter().expect("local test context");
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
        let proof = crate::Entered::<Local>::enter().expect("local test context");
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
    fn retained_entry_is_refused_after_context_exit_without_consuming_payload() {
        let proof = crate::Entered::<Local>::enter().expect("local test context");
        let cell = ContextCell::new(Some(7));
        RUNNING.with(|running| running.set(false));
        let refused = cell.with(&proof, Option::take);
        let reentry = crate::Entered::<Local>::enter();
        RUNNING.with(|running| running.set(true));
        assert_eq!(refused, None);
        assert!(reentry.is_none());
        assert_eq!(cell.with(&proof, Option::take), Some(Some(7)));
    }

    #[test]
    fn nested_access_refuses_consumption_of_an_owned_payload() {
        let proof = crate::Entered::<Local>::enter().expect("local test context");
        let cell = ContextCell::new(Some(std::string::String::from("retained")));
        assert_eq!(
            cell.with(&proof, |_| {
                assert_eq!(cell.with(&proof, Option::take), None);
            }),
            Some(())
        );
        assert_eq!(
            cell.with(&proof, Option::take).flatten().as_deref(),
            Some("retained")
        );
    }

    #[test]
    fn unwinding_ends_the_borrow() {
        let proof = crate::Entered::<Local>::enter().expect("local test context");
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
