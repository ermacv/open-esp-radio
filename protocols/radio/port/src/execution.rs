//! Composition-selected execution of a port's independently borrowed driver.

use core::future::Future;

use crate::{DriverExit, DriverLoan, PortBackend, Retire};

/// The installation's single driver invocation, before it starts polling.
///
/// Only the lifecycle constructs this task. Moving it transfers its exclusive
/// driver loan and retirement receiver; it neither starts another installation
/// nor exposes the loan for replacement. Its borrowed backing must outlive the
/// selected execution and all polls, including cancellation cleanup.
#[must_use = "execute the driver and join its proved exit"]
pub struct PortDriver<'scope, 'id, B: PortBackend> {
    pub(crate) loan: DriverLoan<'scope, B::Driver>,
    pub(crate) held: &'scope B::Held<'id>,
    pub(crate) retire: Retire<'scope>,
}

impl<'scope, 'id, B: PortBackend> PortDriver<'scope, 'id, B> {
    /// Enter the backend's driver in the context chosen by the composition.
    /// The returned future ends only this loan; the lifecycle retains owners.
    pub fn run(self) -> impl Future<Output = DriverExit<B::State, B::Fault>> + use<'scope, 'id, B> {
        B::drive(self.loan, self.held, self.retire)
    }
}

/// Select where an installation's driver is polled and join its exit.
///
/// The body and lifecycle poll the returned join future in their own context.
/// An implementation may poll the driver elsewhere only while retaining its
/// scoped borrow. Dropping the join must end every possible poll before that
/// borrow ends. An ordinary static spawn cannot provide this guarantee.
///
/// Portable drivers and contexts have no universal `Send` bound. A composition
/// crossing execution contexts establishes the bounds for its own backend and
/// transfers only its owned queue endpoints and execution state.
pub trait PortExecution<B: PortBackend>: Sized {
    /// Consume the one invocation and return its cancellation-safe join.
    fn start<'scope, 'id>(
        self,
        driver: PortDriver<'scope, 'id, B>,
    ) -> impl Future<Output = DriverExit<B::State, B::Fault>> + use<'scope, 'id, B, Self>;
}

/// Explicit cooperative execution of the driver beside the body.
///
/// Host models select this policy, including models with non-`Send` contexts.
/// It establishes no synchronization for crossing execution contexts.
pub struct LocalExecution;

impl LocalExecution {
    /// Register a local slot before publishing its frontend to `body`.
    ///
    /// The returned outer future belongs to P2 and outlives the inner body
    /// future. It joins or quarantines its P2 registration before returning.
    /// Destroying that outer runner also occurs on P2; cancelling an inner
    /// frontend future only publishes an abandonment request. No `Send` bound
    /// is needed for a model whose entire hardware world is local.
    pub fn scope<'s, Id: crate::SlotIdentity + 's, S: crate::SlotOwner<Id> + 's, R>(
        self,
        slot: &'s mut crate::PortSlot<Id, S>,
        owner: S,
        body: impl AsyncFnOnce(crate::SlotFrontend<'s, Id, S>) -> R,
    ) -> Result<impl Future<Output = R>, S> {
        crate::slot::run_local(slot, owner, body)
    }
}

impl<B: PortBackend> PortExecution<B> for LocalExecution {
    fn start<'scope, 'id>(
        self,
        driver: PortDriver<'scope, 'id, B>,
    ) -> impl Future<Output = DriverExit<B::State, B::Fault>> + use<'scope, 'id, B> {
        driver.run()
    }
}
