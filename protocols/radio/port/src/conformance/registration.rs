//! Model-only P2 cleanup policy and guard for lifecycle conformance tests.
use crate::*;
use core::future::Future;

/// Simulated hardware owner's policy, excluded from production sans-IO code.
pub trait MockCleanup<Id: SlotIdentity>: SlotOwner<Id> {
    /// Observe cancellation and clean up the model world without releasing backing.
    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's;
}

/// Model-owned guard; actual drivers implement their own P2 guard.
pub struct MockRegistration<'s, Id: SlotIdentity + 's, S: MockCleanup<Id> + 's> {
    raw: SlotRegistration<'s, Id, S>,
}
impl<'s, Id: SlotIdentity + 's, S: MockCleanup<Id> + 's> MockRegistration<'s, Id, S> {
    /// Own the model registration before publishing its frontend or future.
    pub fn new(raw: SlotRegistration<'s, Id, S>) -> Self {
        Self { raw }
    }
    /// Execute the simulated owner's cleanup once.
    pub fn cleanup(&mut self) -> bool {
        self.cleanup_cause(None)
    }
    fn cleanup_cause(&mut self, cause: Option<LifecycleError>) -> bool {
        let observed = self.raw.observation();
        let Some(authority) = self.raw.begin_failure() else {
            return false;
        };
        let cause = match cause {
            Some(cause) => cause,
            None => match self.raw.failure() {
                Some(cause) => cause,
                None => LifecycleError::Cancelled,
            },
        };
        let (outcome, cause) = match self.raw.owner_parts(&authority) {
            Ok((owner, invocation)) => (owner.abandon(invocation, authority), cause),
            Err(_) => (
                Abandon::Unconfirmed(authority),
                match cause {
                    LifecycleError::InvalidState(_) => cause,
                    _ => LifecycleError::InvalidState(observed),
                },
            ),
        };
        self.raw.publish_failure(cause, outcome);
        true
    }
    /// Drive with the guard retained through cancellation, even before first poll.
    pub fn run(
        mut self,
    ) -> crate::DriverFuture<impl Future<Output = Result<SlotCompletion<'s, Id>, LifecycleError>>>
    {
        crate::DriverFuture::new(async move {
            let result = self.raw.drive().await;
            if let Err(cause) = result {
                self.cleanup_cause(Some(cause));
            }
            result
        })
    }
}
impl<Id: SlotIdentity, S: MockCleanup<Id>> Drop for MockRegistration<'_, Id, S> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Construct both model task loans before entering local execution.
pub fn guarded<'s, Id: SlotIdentity + 's, S: MockCleanup<Id> + 's>(
    (frontend, registration): (SlotFrontend<'s, Id, S>, SlotRegistration<'s, Id, S>),
) -> (
    SlotFrontend<'s, Id, S>,
    impl Future<Output = Result<SlotCompletion<'s, Id>, LifecycleError>>,
) {
    (frontend, MockRegistration::new(registration).run())
}
