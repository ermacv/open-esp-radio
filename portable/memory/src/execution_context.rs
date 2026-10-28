//! Checked entry to an execution context, independent of its scheduler.

use crate::ContextProof;
use core::marker::PhantomData;

/// The execution identity that admits context-local memory access.
///
/// # Safety
/// If `Sharing: Sync`, `running` must be true only in one non-concurrent
/// execution context for this concrete type, excluding preempting handlers
/// and other cores. If `Sharing` is not `Sync`, cells remain on their creating
/// thread and `running` admits that thread's local execution. Implementations
/// must not use scheduling or lifecycle assumptions as a substitute for this
/// memory exclusion. Checks must remain valid until control leaves the caller.
#[allow(unsafe_code, reason = "execution owners establish memory exclusion")]
// SAFETY: implementations establish the context identity/exclusion above;
// Entered rechecks it before ContextCell touches its non-atomic borrow flag.
pub unsafe trait ExecutionContext: 'static {
    type Sharing: 'static;
    fn running() -> bool;
}

/// A checked, non-transferable proof of execution in `X`.
///
/// Its private field prevents construction without checking `X`. Cell access
/// checks `X` again, so retaining a proof does not extend its admission.
pub struct Entered<X: ExecutionContext>(PhantomData<(*mut (), X)>);

impl<X: ExecutionContext> Entered<X> {
    pub fn enter() -> Option<Self> {
        X::running().then_some(Self(PhantomData))
    }
}

#[allow(unsafe_code, reason = "checked execution entry proves cell access")]
// SAFETY: construction and each cell access check X::running. X's contract
// excludes concurrency for shareable cells; local cells cannot leave their
// constructing thread. The raw-pointer marker keeps the proof !Send/!Sync.
unsafe impl<X: ExecutionContext> ContextProof for Entered<X> {
    type Sharing = X::Sharing;
    fn is_current(&self) -> bool {
        X::running()
    }
}

/// An execution context that cannot be entered.
pub enum Never {}
#[allow(unsafe_code, reason = "an unavailable context admits no cell access")]
// SAFETY: running is always false, so neither entry nor access is admitted.
unsafe impl ExecutionContext for Never {
    type Sharing = ();
    fn running() -> bool {
        false
    }
}

/// The constructing thread's local execution, for host signal owners.
/// Cells using this context are !Send and !Sync; only their atomic raisers
/// may cross threads. This context supplies no globally shareable proof.
#[cfg(any(test, feature = "host-context"))]
pub enum HostContext {}
#[cfg(any(test, feature = "host-context"))]
#[allow(
    unsafe_code,
    reason = "non-shareable cells stay on their creating thread"
)]
// SAFETY: Sharing is !Sync and Entered is !Send, so a ContextCell using this
// proof cannot move to or be referenced from another thread. Reentrant
// access on its one thread is refused by ContextCell's borrow flag.
unsafe impl ExecutionContext for HostContext {
    type Sharing = *mut ();
    fn running() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContextCell;
    #[test]
    fn unavailable_context_cannot_be_entered() {
        assert!(Entered::<Never>::enter().is_none());
    }
    // context_cell tests exercise checked entry and refusal after exit;
    // this host case also exercises the host-specific local sharing marker.
    #[test]
    fn nested_access_retains_payload_until_the_outer_borrow_ends() {
        let proof = Entered::<HostContext>::enter().expect("local context");
        let cell = ContextCell::new(Some(7));
        assert_eq!(
            cell.with(&proof, |_| {
                assert_eq!(cell.with(&proof, Option::take), None);
            }),
            Some(())
        );
        assert_eq!(cell.with(&proof, Option::take), Some(Some(7)));
    }
}
