//! T7's dated terminal commit. Observers run after the engine has returned.
use crate::{
    analog::temperature::PhyTemperatureOutcome,
    executor::step::{self, Failure, Registers},
    target_executor::PhyTargetPortError,
    tracking::{
        observation::{EngineTiming, ReadinessTrace},
        parameters::{PhyParamTrackingCompletion, PhyParamTrackingTemperatureTransition},
    },
};
use oer_cooperate::CooperativeTimer as Timer;

type Result =
    core::result::Result<(PhyParamTrackingCompletion, PhyTemperatureOutcome), PhyTargetPortError>;
pub(super) async fn temperature<D: crate::PhyShortDelay, R: Registers, T: Timer>(
    timer: &T,
    mut child: PhyParamTrackingTemperatureTransition<'_>,
    registers: &mut R,
    measured: bool,
    trace: Option<&mut ReadinessTrace>,
) -> (Result, EngineTiming) {
    let started = Some(timer.now().as_micros());
    let mut timing = EngineTiming::default();
    let result = match step::temperature::sample::<D, _, _>(
        child.transition_mut(),
        registers,
        timer,
        measured.then_some(&mut timing),
        trace,
    )
    .await
    {
        Ok(outcome) => child
            .commit_observed(started, Some(timer.now().as_micros()))
            .map(|completion| (completion, outcome))
            .map_err(|_| PhyTargetPortError::UnexpectedBinding),
        Err(error) => Err(port_error(error)),
    };
    (result, timing)
}
pub(crate) fn port_error(error: Failure) -> PhyTargetPortError {
    match error {
        Failure::Target(error) => error,
        Failure::DeadlineOverflow => PhyTargetPortError::DeadlineOverflow,
        Failure::TimedOut => PhyTargetPortError::HardwareEdgeTimedOut,
        Failure::Binding => PhyTargetPortError::UnexpectedBinding,
    }
}
