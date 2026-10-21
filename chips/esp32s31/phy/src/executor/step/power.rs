//! T2/T4 own their state in the shared P1 driver; gain publication has its own poll.
use super::{Failure, Io, Registers, Step};
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

pin_project_lite::pin_project! {
    struct Power<F> { #[pin] inner: F }
}
impl<F: super::Engine> core::future::Future for Power<F> {
    type Output = (Result<PhyParamTrackingCompletion, Failure>, EngineTiming);
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        let mut this = self.project();
        match this.inner.as_mut().poll(cx) {
            core::task::Poll::Pending => core::task::Poll::Pending,
            core::task::Poll::Ready(result) => {
                core::task::Poll::Ready(this.inner.finish_power(result))
            }
        }
    }
}

pub(crate) fn run_param_tx_power<
    'a,
    D: crate::executor::wait::PhyShortDelay + 'a,
    R: Registers,
    T: Timer,
>(
    timer: &'a T,
    child: PhyParamTrackingTxPowerTransition<'a>,
    registers: &'a mut R,
    measured: bool,
) -> impl core::future::Future<Output = (Result<PhyParamTrackingCompletion, Failure>, EngineTiming)> + 'a
{
    Power {
        inner: super::driver::<D, _, _>(
            super::Input::Power { child, measured },
            registers,
            timer,
            None,
            None,
        ),
    }
}

/// Exactly one BBPLL edge, full force/settle pair, or full gain image/publish.
/// No gain image survives a Pending edge and no timer represents Runnable work.
#[inline(never)]
pub(super) fn advance<D: crate::executor::wait::PhyShortDelay>(
    io: &mut Io<'_, '_>,
    child: &mut PhyParamTrackingTxPowerTransition<'_>,
) -> Result<Step, Failure> {
    let (transition, state) = child.parts();
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
