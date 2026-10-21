//! T2/T4 share one P1 job and keep gain publication inside its own poll.
use super::{Failure, Io, Job, Registers, Step, drive};
use crate::{
    analog::pbus::PhyForceTxRxAction,
    state::PhyState,
    tracking::{
        observation::EngineTiming,
        parameters::{PhyParamTrackingCompletion, PhyParamTrackingTxPowerTransition},
        power::{PhyTxPowerTrackingAction as Action, PhyTxPowerTrackingTransition},
    },
};
use oer_time::Timer;

pub(crate) async fn run_param_tx_power<
    D: crate::executor::wait::PhyShortDelay,
    R: Registers,
    T: Timer,
>(
    timer: &T,
    mut child: PhyParamTrackingTxPowerTransition<'_>,
    registers: &mut R,
    measured: bool,
) -> (Result<PhyParamTrackingCompletion, Failure>, EngineTiming) {
    let mut timing = EngineTiming::default();
    let result = {
        let (transition, state) = child.parts();
        match Job::power(transition, state) {
            Err(error) => Err(error),
            Ok(mut job) => {
                drive::<D, _, _>(
                    &mut job,
                    registers,
                    timer,
                    measured.then_some(&mut timing),
                    None,
                    None,
                )
                .await
            }
        }
    };
    let result = result.and_then(|()| child.commit().map_err(|_| Failure::Binding));
    (result, timing)
}

/// Exactly one BBPLL edge, full force/settle pair, or full gain image/publish.
/// No gain image survives a Pending edge and no timer represents Runnable work.
#[inline(never)]
pub(super) fn advance<D: crate::executor::wait::PhyShortDelay>(
    io: &mut Io<'_, '_>,
    transition: &mut PhyTxPowerTrackingTransition,
    state: &PhyState,
) -> Result<Step, Failure> {
    if matches!(transition.action(), Action::Complete(_)) {
        return Ok(Step::Complete);
    }
    execute::<D>(io, transition, state)?;
    // Configure and its mandatory 1-us minimum remain one indivisible atom.
    // The next configure phase and the gain publisher each get a fresh poll.
    if matches!(
        transition.action(),
        Action::ForceTxRx(PhyForceTxRxAction::DelayMicros { .. })
    ) {
        execute::<D>(io, transition, state)?;
    }
    Ok(Step::Runnable)
}

fn execute<D: crate::executor::wait::PhyShortDelay>(
    io: &mut Io<'_, '_>,
    transition: &mut PhyTxPowerTrackingTransition,
    state: &PhyState,
) -> Result<(), Failure> {
    let binding = crate::tracking::power::PhyTxPowerTrackingExternalBinding::lower(
        transition.action(),
        state,
    )
    .map_err(|_| Failure::Binding)?;
    if let Some(micros) = binding.delay_micros()
        && !D::settle_micros(micros)
    {
        #[cfg(target_arch = "riscv32")]
        return Err(Failure::Target(
            crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable,
        ));
        #[cfg(not(target_arch = "riscv32"))]
        return Err(Failure::Binding);
    }
    let completion = io.power(binding)?;
    transition.advance(completion).map_err(|_| Failure::Binding)
}
