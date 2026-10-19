//! Shared affine controller-time lifecycle for pre-publication role preparation.
//!
//! Role adapters retain their semantic phase and provide only a lossless
//! rollback function. This engine owns request, bounded recheck, explicit
//! cancellation, and abandoned-request drain. A controller becomes reusable
//! only after the exact cancelled request reports `Drained`.

#![forbid(unsafe_code)]

use crate::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimePendingCore, ControllerTimePendingCoreStep,
        ControllerTimePendingOrphanStep, ControllerTimePendingOwner,
        ControllerTimePendingOwnerStep, ControllerTimeRequest,
    },
    scheduler::ControllerTimeAcquisitionError,
};

/// Minimal controller-time operations required by the shared preparation engine.
pub trait TimedPreparationController: ControllerTimePendingOwner {
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<ControllerTimeRequest, ControllerTimeAcquisitionError>;
}

/// Role-specific result of returning one unpublished graph to its runtime.
///
/// This is deliberately not `Result`: restoration failure retains an affine
/// owner and is a sealed lifecycle outcome, not a value-level error.
#[derive(Debug)]
pub enum TimedPreparationRollbackOutcome<R> {
    Restored,
    FailStop(R),
}

impl<R> TimedPreparationRollbackOutcome<R> {
    fn into_failure(self) -> Option<R> {
        match self {
            Self::Restored => None,
            Self::FailStop(owner) => Some(owner),
        }
    }
}

type TimedPreparationRollback<C, P, R> = fn(&mut C, P) -> TimedPreparationRollbackOutcome<R>;

#[derive(Debug)]
struct TimedPreparationOwner<C, P, R>
where
    C: TimedPreparationController,
{
    controller: C,
    phase: Option<P>,
    rollback: TimedPreparationRollback<C, P, R>,
    cancelled: Option<TimedPreparationRollbackOutcome<R>>,
}

impl<C, P, R> ControllerTimePendingOwner for TimedPreparationOwner<C, P, R>
where
    C: TimedPreparationController,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        self.controller.recheck_owned_controller_time(request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        let result = self.controller.cancel_owned_controller_time(request);
        let Some(phase) = self.phase.take() else {
            return Err(ControllerTimeEventError::RequestMismatch);
        };
        self.cancelled = Some((self.rollback)(&mut self.controller, phase));
        result
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        self.controller.drain_orphan_controller_time()
    }
}

/// Closed permanent-fault class shared by every timed role preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimedPreparationFailStopCause {
    ControllerTime(ControllerTimeAcquisitionError),
    Rollback,
    PhaseOwnership,
}

/// Sealed controller and optional rollback owner after an unsafe transition.
#[must_use = "retain the complete fail-stop owner"]
#[derive(Debug)]
pub struct TimedPreparationFailStop<C, R> {
    cause: TimedPreparationFailStopCause,
    controller: C,
    rollback: Option<R>,
}

impl<C, R> TimedPreparationFailStop<C, R> {
    pub const fn cause(&self) -> TimedPreparationFailStopCause {
        self.cause
    }

    pub fn into_parts(self) -> (C, Option<R>) {
        (self.controller, self.rollback)
    }
}

/// One exact in-flight role preparation request.
#[must_use = "recheck or explicitly cancel the timed preparation"]
#[derive(Debug)]
pub struct TimedPreparationPending<C, P, R>
where
    C: TimedPreparationController,
{
    core: ControllerTimePendingCore<TimedPreparationOwner<C, P, R>>,
}

/// Result of one bounded sample observation.
#[must_use = "retain Waiting, consume Ready, or retain FailStop"]
pub enum TimedPreparationStep<C, P, R>
where
    C: TimedPreparationController,
{
    Waiting(TimedPreparationPending<C, P, R>),
    Ready {
        controller: C,
        phase: P,
        sample: ControllerTimeSample,
    },
    FailStop(TimedPreparationFailStop<C, R>),
}

/// Controller whose role graph is restored while its abandoned request drains.
#[must_use = "drain the exact abandoned request before controller reuse"]
#[derive(Debug)]
pub struct TimedPreparationCancellationPending<C>
where
    C: TimedPreparationController,
{
    controller: C,
}

/// One bounded abandoned-request observation.
#[must_use = "retain Waiting, consume Recovered, or retain FailStop"]
pub enum TimedPreparationCancellationStep<C, R>
where
    C: TimedPreparationController,
{
    Waiting(TimedPreparationCancellationPending<C>),
    Recovered(C),
    FailStop(TimedPreparationFailStop<C, R>),
}

const fn event_error(error: ControllerTimeEventError) -> ControllerTimeAcquisitionError {
    match error {
        ControllerTimeEventError::RequestMismatch => {
            ControllerTimeAcquisitionError::RequestMismatch
        }
        ControllerTimeEventError::OwnershipLost => ControllerTimeAcquisitionError::OwnershipLost,
        ControllerTimeEventError::Faulted => ControllerTimeAcquisitionError::Faulted,
    }
}

impl<C, P, R> TimedPreparationPending<C, P, R>
where
    C: TimedPreparationController,
{
    pub fn begin(
        mut controller: C,
        phase: P,
        rollback: TimedPreparationRollback<C, P, R>,
    ) -> Result<Self, TimedPreparationFailStop<C, R>> {
        let request = match controller.request_timed_preparation_sample() {
            Ok(request) => request,
            Err(error) => {
                let rollback = rollback(&mut controller, phase).into_failure();
                return Err(TimedPreparationFailStop {
                    cause: TimedPreparationFailStopCause::ControllerTime(error),
                    controller,
                    rollback,
                });
            }
        };
        Ok(Self {
            core: ControllerTimePendingCore::new(
                TimedPreparationOwner {
                    controller,
                    phase: Some(phase),
                    rollback,
                    cancelled: None,
                },
                request,
            ),
        })
    }

    pub fn recheck(self) -> TimedPreparationStep<C, P, R> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(ControllerTimePendingCoreStep::Waiting(core)) => {
                return TimedPreparationStep::Waiting(Self { core });
            }
            Ok(ControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let Some(phase) = owner.phase.take() else {
                    return TimedPreparationStep::FailStop(TimedPreparationFailStop {
                        cause: TimedPreparationFailStopCause::PhaseOwnership,
                        controller: owner.controller,
                        rollback: None,
                    });
                };
                let rollback = (owner.rollback)(&mut owner.controller, phase).into_failure();
                return TimedPreparationStep::FailStop(TimedPreparationFailStop {
                    cause: TimedPreparationFailStopCause::ControllerTime(event_error(error)),
                    controller: owner.controller,
                    rollback,
                });
            }
        };
        let Some(phase) = owner.phase.take() else {
            return TimedPreparationStep::FailStop(TimedPreparationFailStop {
                cause: TimedPreparationFailStopCause::PhaseOwnership,
                controller: owner.controller,
                rollback: None,
            });
        };
        TimedPreparationStep::Ready {
            controller: owner.controller,
            phase,
            sample,
        }
    }

    pub fn cancel(
        self,
    ) -> Result<TimedPreparationCancellationPending<C>, TimedPreparationFailStop<C, R>> {
        match self.core.cancel() {
            Ok(mut owner) => match owner.cancelled.take() {
                Some(TimedPreparationRollbackOutcome::Restored) => {
                    Ok(TimedPreparationCancellationPending {
                        controller: owner.controller,
                    })
                }
                Some(TimedPreparationRollbackOutcome::FailStop(rollback)) => {
                    Err(TimedPreparationFailStop {
                        cause: TimedPreparationFailStopCause::Rollback,
                        controller: owner.controller,
                        rollback: Some(rollback),
                    })
                }
                None => Err(TimedPreparationFailStop {
                    cause: TimedPreparationFailStopCause::PhaseOwnership,
                    controller: owner.controller,
                    rollback: None,
                }),
            },
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                Err(TimedPreparationFailStop {
                    cause: TimedPreparationFailStopCause::ControllerTime(event_error(error)),
                    controller: owner.controller,
                    rollback: owner
                        .cancelled
                        .take()
                        .and_then(TimedPreparationRollbackOutcome::into_failure),
                })
            }
        }
    }
}

impl<C> TimedPreparationCancellationPending<C>
where
    C: TimedPreparationController,
{
    pub fn recheck<R>(mut self) -> TimedPreparationCancellationStep<C, R> {
        match self.controller.drain_orphan_controller_time() {
            Ok(ControllerTimePendingOrphanStep::Waiting) => {
                TimedPreparationCancellationStep::Waiting(self)
            }
            Ok(ControllerTimePendingOrphanStep::Drained) => {
                TimedPreparationCancellationStep::Recovered(self.controller)
            }
            Ok(ControllerTimePendingOrphanStep::Idle) => {
                TimedPreparationCancellationStep::FailStop(TimedPreparationFailStop {
                    cause: TimedPreparationFailStopCause::PhaseOwnership,
                    controller: self.controller,
                    rollback: None,
                })
            }
            Err(error) => TimedPreparationCancellationStep::FailStop(TimedPreparationFailStop {
                cause: TimedPreparationFailStopCause::ControllerTime(event_error(error)),
                controller: self.controller,
                rollback: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
