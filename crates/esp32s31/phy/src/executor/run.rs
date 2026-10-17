//! Port traits and async drivers for the registration and tracking graphs.
//!
//! Only the crate's target ports and validation probes drive these graphs;
//! protocol code reaches them through the registered owners.

use crate::tracking::observation::{Event, Operation};
use core::future::Future;

use crate::calibration::registration::{
    PhyRegisterBindingError, PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterFailure,
    PhyRegisterLocalStep, PhyRegisterOutcome, PhyRegisterTransition, PhyRegisterTransitionError,
};

use crate::tracking::calibration::CALIBRATION_TRACKING_ACTION_LIMIT;
const PARAM_TRACKING_PARENT_EDGE_LIMIT: u8 = 16;

/// Async completion boundary for one identity-bound PHY hardware operation.
///
/// Implementations must consume the binding exactly once. MMIO-only bindings
/// may return `Ready` after their finite register sequence. Settling and
/// readiness execution follows the target timing contract: bounded direct
/// observations and short synchronous settles may complete in the current
/// poll; longer waits use a Rust-owned timer or interrupt. No implementation
/// may use an unbounded polling loop, allocate, or invoke a vendor/ROM radio
/// parent. Hardware ownership remains retained across either wait mechanism.
///
/// The returned future is an associated, statically dispatched type. No
/// `Box`, trait object, or allocator is required by this interface.
pub trait PhyRegisterPort {
    type Error;

    fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> impl Future<Output = Result<PhyRegisterCompletion, Self::Error>> + '_;
}

/// Async completion boundary for one complete periodic-calibration child.
///
/// The port receives the retained live-state owner rather than a forgeable
/// action/completion pair. Opaque child proofs can therefore be produced only
/// by driving the selected typed child to its terminal edge.
pub trait PhyCalibrationTrackingPort {
    type Error;

    fn observe(&mut self, _operation: Operation, _event: Event) {}

    fn complete<'port, 'state>(
        &'port mut self,
        transition: &'port mut crate::tracking::parameters::PhyParamTrackingCalibrationTransition<
            'state,
        >,
    ) -> impl Future<
        Output = Result<
            crate::tracking::calibration::PhyCalibrationTrackingCompletion,
            Self::Error,
        >,
    > + 'port;
}

/// Async completion boundary for one complete outer periodic-tracking child.
pub trait PhyParamTrackingPort {
    type Error;

    fn observe(&mut self, _operation: Operation, _event: Event) {}

    fn complete<'port>(
        &'port mut self,
        pending: &'port mut crate::state::client::PhyPendingTracking,
        state: &'port mut crate::PhyState,
    ) -> impl Future<
        Output = Result<crate::tracking::parameters::PhyParamTrackingCompletion, Self::Error>,
    > + 'port;
}

/// Failure reported while driving a caller-owned registration transition.
///
/// This generic executor is also the host-model boundary: safe test ports may
/// synthesize completions without touching hardware. Consequently, completing
/// this runner never creates target-registration authority. A completed model
/// transition can recover only ordinary state through
/// `PhyRegisterTransition::into_model_parts`.
///
/// The transition is borrowed rather than consumed. After any error, the
/// caller still owns it and can inspect its state. After a terminal radio
/// failure, `PhyRegisterTransition::into_failed_parts` recovers the ordinary
/// PHY owner and retry input. On ESP32-S31 targets, only the concrete opaque
/// target-attempt runner can yield [`crate::RegisteredPhyState`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterRunError<E> {
    /// The stage of the action that could not be lowered identifies the
    /// missing hardware binding without requiring an instrumented build.
    Lowering {
        stage: crate::calibration::registration::PhyRegisterStage,
        error: PhyRegisterBindingError,
    },
    Port(E),
    Transition(PhyRegisterTransitionError),
    Radio(PhyRegisterFailure),
}

/// Failure while driving the bounded parent of `phy_cal_param_track`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingRunError<E> {
    Port(E),
    Transition(crate::tracking::calibration::PhyCalibrationTrackingTransitionError),
    Radio(crate::tracking::calibration::PhyCalibrationTrackingFailure),
    ParentEdgeLimit,
}

/// Failure while driving one complete `phy_param_track_tot` request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingRunError<E> {
    Port(E),
    Transition(crate::tracking::parameters::PhyParamTrackingTransitionError),
    ParentEdgeLimit,
}

/// Drive `register_chipv7_phy` as a Rust async state machine or host model.
///
/// Local arithmetic and ownership transitions run synchronously because they
/// cannot wait on hardware. Every external edge is first lowered to an
/// exhaustive source-owned binding and then awaited through `port`.
///
/// `port` is a caller-provided completion oracle. This function deliberately
/// returns only a value outcome and cannot mint hardware-registration proof.
pub async fn run_phy_register<P: PhyRegisterPort>(
    transition: &mut PhyRegisterTransition,
    port: &mut P,
) -> Result<PhyRegisterOutcome, PhyRegisterRunError<P::Error>> {
    loop {
        match transition
            .step_local()
            .map_err(PhyRegisterRunError::Transition)?
        {
            PhyRegisterLocalStep::StateAdvanced => {}
            PhyRegisterLocalStep::External(action) => {
                let binding = PhyRegisterExternalBinding::lower(action).map_err(|error| {
                    PhyRegisterRunError::Lowering {
                        stage: action.stage(),
                        error,
                    }
                })?;
                let completion = port
                    .complete(binding)
                    .await
                    .map_err(PhyRegisterRunError::Port)?;
                transition
                    .advance_external(completion)
                    .map_err(PhyRegisterRunError::Transition)?;
            }
            PhyRegisterLocalStep::Complete(outcome) => return Ok(outcome),
            PhyRegisterLocalStep::Failed(failure) => {
                return Err(PhyRegisterRunError::Radio(failure));
            }
        }
    }
}

/// Drive one complete periodic calibration parent through identity-bound
/// children without exposing its live [`crate::PhyState`] owner.
///
/// Terminal success leaves `transition` ready for its consuming `commit`.
/// Every error leaves the exact transition and state borrow with the caller;
/// the surrounding PHY-client owner must then enter its fail-stop state.
pub async fn run_phy_calibration_tracking<P: PhyCalibrationTrackingPort>(
    transition: &mut crate::tracking::parameters::PhyParamTrackingCalibrationTransition<'_>,
    port: &mut P,
) -> Result<(), PhyCalibrationTrackingRunError<P::Error>> {
    for _ in 0..CALIBRATION_TRACKING_ACTION_LIMIT {
        match transition.action() {
            crate::tracking::calibration::PhyCalibrationTrackingAction::Complete(_) => {
                return Ok(());
            }
            crate::tracking::calibration::PhyCalibrationTrackingAction::Failed(failure) => {
                return Err(PhyCalibrationTrackingRunError::Radio(failure));
            }
            action => {
                let operation = Operation::calibration(action);
                if let Some(operation) = operation {
                    port.observe(operation, Event::Started);
                }
                let result = match port.complete(transition).await {
                    Ok(completion) => transition
                        .advance(completion)
                        .map_err(PhyCalibrationTrackingRunError::Transition),
                    Err(error) => Err(PhyCalibrationTrackingRunError::Port(error)),
                };
                if let Some(operation) = operation {
                    port.observe(
                        operation,
                        if result.is_ok() {
                            Event::Completed
                        } else {
                            Event::Failed
                        },
                    );
                }
                result?;
            }
        }
    }
    Err(PhyCalibrationTrackingRunError::ParentEdgeLimit)
}

/// Drive every selected child of one unique pending PHY-client request.
///
/// `EnterCritical`/`ExitCritical` are ownership boundaries rather than CPU
/// interrupt masking: the affine [`crate::state::client::PhyPendingTracking`]
/// owner excludes a concurrent radio transition while asynchronous hardware
/// timers and interrupts remain able to make progress.
///
/// On error, callers must consume `pending` through its fail-stop path. A
/// terminal success leaves it ready for `into_owner`.
pub async fn run_phy_param_tracking<P: PhyParamTrackingPort>(
    pending: &mut crate::state::client::PhyPendingTracking,
    state: &mut crate::PhyState,
    port: &mut P,
) -> Result<crate::tracking::parameters::PhyParamTrackingOutcome, PhyParamTrackingRunError<P::Error>>
{
    port.observe(Operation::Tracking, Event::Started);
    for _ in 0..PARAM_TRACKING_PARENT_EDGE_LIMIT {
        match pending.action() {
            crate::tracking::parameters::PhyParamTrackingAction::Complete(outcome) => {
                port.observe(Operation::Tracking, Event::Completed);
                return Ok(outcome);
            }
            action => {
                let operation = Operation::parameter(action);
                if let Some(operation) = operation {
                    port.observe(operation, Event::Started);
                }
                let result = match port.complete(pending, state).await {
                    Ok(completion) => pending
                        .advance(completion)
                        .map_err(PhyParamTrackingRunError::Transition),
                    Err(error) => Err(PhyParamTrackingRunError::Port(error)),
                };
                if let Some(operation) = operation {
                    port.observe(
                        operation,
                        if result.is_ok() {
                            Event::Completed
                        } else {
                            Event::Failed
                        },
                    );
                }
                if let Err(error) = result {
                    port.observe(Operation::Tracking, Event::Failed);
                    return Err(error);
                }
            }
        }
    }
    port.observe(Operation::Tracking, Event::Failed);
    Err(PhyParamTrackingRunError::ParentEdgeLimit)
}
