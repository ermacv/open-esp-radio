//! The existing temperature graph chooses transport jobs; conversion and
//! range publication never become part of the transport dispatcher.
use super::{Failure, I2cTiming, Job, Registers, drive};
use crate::{
    analog::temperature::{
        PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureExternalBinding,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
    tracking::observation::{EngineTiming, ReadinessTrace},
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use oer_time::Timer;

pin_project_lite::pin_project! {
    struct Sample<F> { #[pin] inner: F }
}
impl<F: Future<Output = Result<PhyTemperatureOutcome, Failure>>> Future for Sample<F> {
    type Output = Result<PhyTemperatureOutcome, Failure>;
    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}
pub(crate) fn sample<'a, D: crate::executor::wait::PhyShortDelay + 'a, R: Registers, T: Timer>(
    transition: &'a mut PhyTemperatureTransition,
    registers: &'a mut R,
    timer: &'a T,
    mut timing: Option<&'a mut EngineTiming>,
    mut trace: Option<&'a mut ReadinessTrace>,
) -> impl Future<Output = Result<PhyTemperatureOutcome, Failure>> + 'a {
    Sample {
        inner: async move {
            loop {
                match transition.action() {
                    PhyTemperatureAction::Complete(outcome) => return Ok(outcome),
                    PhyTemperatureAction::Failed(_) => return Err(Failure::Binding),
                    _ => {}
                }
                let completion = match PhyTemperatureExternalBinding::lower(transition.action())
                    .map_err(|_| Failure::Binding)?
                {
                    PhyTemperatureExternalBinding::I2c(mut binding) => {
                        let mut job = Job::i2c(
                            binding.transaction_mut(),
                            timer.now(),
                            I2cTiming::Temperature,
                        )?;
                        drive::<D, _, _>(
                            &mut job,
                            registers,
                            timer,
                            timing.as_deref_mut(),
                            trace.as_deref_mut(),
                            None,
                        )
                        .await?;
                        binding.into_completion().map_err(|_| Failure::Binding)?
                    }
                    PhyTemperatureExternalBinding::Sample(_) => {
                        let mut job = Job::code(timer.now())?;
                        drive::<D, _, _>(
                            &mut job,
                            registers,
                            timer,
                            timing.as_deref_mut(),
                            trace.as_deref_mut(),
                            None,
                        )
                        .await?;
                        let value = job.code_value()?;
                        PhyTemperatureCompletion::CodeSampled { value }
                    }
                };
                transition
                    .advance(completion)
                    .map_err(|_| Failure::Binding)?;
            }
        },
    }
}
