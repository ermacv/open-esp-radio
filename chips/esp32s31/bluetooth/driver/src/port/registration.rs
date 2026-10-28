//! Hardware-owned P2 cancellation guard for the port registration.
use super::*;
use oer_radio_port::{
    LifecycleError, SlotCompletion, SlotFrontend, SlotRegisterError, SlotRegistration,
};

/// Owns mandatory P2 cleanup independently of the portable driver loan.
#[must_use = "retain the hardware owner's P2 cleanup guard"]
pub struct Registration<'s, Id: SlotIdentity + 's, D: Device + 's, X: 's> {
    raw: SlotRegistration<'s, Id, Backend<Id, D, X>>,
}

/// Rejected input or initialization failure with the hardware owner's guard.
pub enum RegistrationError<
    's,
    Id: SlotIdentity + 's,
    D: Device + 's,
    X: 's,
    Rejected = Backend<Id, D, X>,
> {
    /// No registration was admitted; retain the rejected input.
    Occupied(Rejected),
    /// A join/storage mismatch preserves the reconstruction and affine proof.
    InvalidState {
        /// Portable source observations.
        cause: LifecycleError,
        /// Unconsumed reconstruction operation.
        rejected: Rejected,
        /// Supplied ordinary join authority.
        joined: oer_radio_port::Joined<Id>,
    },
    /// Construction failed after placement; cleanup stays on P2.
    Initialization {
        /// Typed construction cause.
        cause: oer_radio_port::SlotInitError,
        /// Hardware-owned cleanup guard.
        registration: Registration<'s, Id, D, X>,
    },
}
impl<'s, Id: SlotIdentity + 's, D: Device + 's, X: 's, R> core::fmt::Debug
    for RegistrationError<'s, Id, D, X, R>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Occupied(_) => f.write_str("Occupied"),
            Self::InvalidState { cause, .. } => f.debug_tuple("InvalidState").field(cause).finish(),
            Self::Initialization { cause, .. } => {
                f.debug_tuple("Initialization").field(cause).finish()
            }
        }
    }
}
impl<'s, Id: SlotIdentity + 's, D: Device + 's, X: 's> Registration<'s, Id, D, X> {
    /// Retain cleanup before the frontend or its driver future is published.
    pub fn new(raw: SlotRegistration<'s, Id, Backend<Id, D, X>>) -> Self {
        Self { raw }
    }

    /// Guard admitted registrations and failed construction in one operation.
    pub fn guard_result<R>(
        result: Result<
            (
                SlotFrontend<'s, Id, Backend<Id, D, X>>,
                SlotRegistration<'s, Id, Backend<Id, D, X>>,
            ),
            SlotRegisterError<'s, Id, Backend<Id, D, X>, R>,
        >,
    ) -> Result<(SlotFrontend<'s, Id, Backend<Id, D, X>>, Self), RegistrationError<'s, Id, D, X, R>>
    {
        match result {
            Ok((frontend, raw)) => Ok((frontend, Self::new(raw))),
            Err(SlotRegisterError::Occupied(rejected)) => {
                Err(RegistrationError::Occupied(rejected))
            }
            Err(SlotRegisterError::InvalidState {
                cause,
                rejected,
                joined,
            }) => Err(RegistrationError::InvalidState {
                cause,
                rejected,
                joined,
            }),
            Err(SlotRegisterError::Initialization {
                cause,
                registration,
            }) => Err(RegistrationError::Initialization {
                cause,
                registration: Self::new(registration),
            }),
        }
    }

    /// Construct the owner's driver future beside a local frontend body.
    pub fn guarded(
        (frontend, raw): (
            SlotFrontend<'s, Id, Backend<Id, D, X>>,
            SlotRegistration<'s, Id, Backend<Id, D, X>>,
        ),
    ) -> (
        SlotFrontend<'s, Id, Backend<Id, D, X>>,
        impl Future<Output = Result<SlotCompletion<'s, Id>, LifecycleError>>,
    ) {
        (frontend, Self::new(raw).run())
    }

    fn cleanup(&mut self, cause: Option<LifecycleError>) {
        let observed = self.raw.observation();
        let Some(authority) = self.raw.begin_failure() else {
            return;
        };
        let cause = match cause {
            Some(cause) => cause,
            None => match self.raw.failure() {
                Some(cause) => cause,
                None => LifecycleError::Cancelled,
            },
        };
        let (outcome, cause) = match self.raw.owner_parts(&authority) {
            Ok((owner, invocation)) => {
                let outcome = (|| {
                    if let Some(invocation) = invocation {
                        invocation.shared.poison(Fault::Abandoned);
                        invocation.wakes.wake_frontend();
                    }
                    if let Some(d) = owner.device.as_mut() {
                        d.quarantine();
                    }
                    // P1 cancellation never supplies synchronous DMA/ISR exclusion proof.
                    Abandon::Unconfirmed(authority)
                })();
                (outcome, cause)
            }
            Err(_) => (
                Abandon::Unconfirmed(authority),
                match cause {
                    LifecycleError::InvalidState(_) => cause,
                    _ => LifecycleError::InvalidState(observed),
                },
            ),
        };
        self.raw.publish_failure(cause, outcome);
    }

    /// Drive under the owner guard, including cancellation before first poll.
    pub fn run(
        mut self,
    ) -> oer_radio_port::DriverFuture<
        impl Future<Output = Result<SlotCompletion<'s, Id>, LifecycleError>>,
    > {
        oer_radio_port::DriverFuture::new(async move {
            let result = self.raw.drive().await;
            if let Err(cause) = result {
                self.cleanup(Some(cause));
            }
            result
        })
    }
}
impl<'s, Id: SlotIdentity + 's, D: Device + 's, X: 's> Drop for Registration<'s, Id, D, X> {
    fn drop(&mut self) {
        self.cleanup(None);
    }
}
