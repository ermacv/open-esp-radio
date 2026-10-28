//! Composition-owned local P2 registration.

use core::future::Future;

/// Explicit cooperative execution of the driver beside the body.
///
/// Host models select this policy, including models with non-`Send` contexts.
/// It establishes no synchronization for crossing execution contexts.
pub struct LocalExecution;

impl LocalExecution {
    /// Run a frontend and its hardware owner's guarded P2 future beside `body`.
    ///
    /// The returned outer future belongs to P2 and outlives the inner body
    /// future. It joins or quarantines its P2 registration before returning.
    /// Destroying that outer runner also occurs on P2; cancelling an inner
    /// frontend future only publishes an abandonment request. No `Send` bound
    /// is needed for a model whose entire hardware world is local.
    pub fn scope<'s, Id: crate::SlotIdentity + 's, S: crate::SlotOwner<Id> + 's, R>(
        self,
        registered: (
            crate::SlotFrontend<'s, Id, S>,
            impl Future<Output = Result<crate::SlotCompletion<'s, Id>, crate::LifecycleError>>,
        ),
        body: impl AsyncFnOnce(crate::SlotFrontend<'s, Id, S>) -> R,
    ) -> impl Future<Output = R> {
        let (frontend, registration) = registered;
        crate::slot::run_registered(frontend, registration, body)
    }
}
