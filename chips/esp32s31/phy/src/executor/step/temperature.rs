//! The existing temperature graph chooses transport jobs; conversion and
//! range publication never become part of the transport dispatcher.
use super::{Failure, Job, Registers, drive};
use crate::{
    analog::temperature::{
        PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureExternalBinding,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
    tracking::observation::EngineTiming,
};
use oer_time::Timer;

pub(crate) async fn sample<R: Registers, T: Timer>(
    transition: &mut PhyTemperatureTransition,
    registers: &mut R,
    timer: &T,
    mut timing: Option<&mut EngineTiming>,
) -> Result<PhyTemperatureOutcome, Failure> {
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
                let mut job = Job::i2c(binding.transaction_mut(), timer.now(), true)?;
                drive(&mut job, registers, timer, timing.as_deref_mut()).await?;
                binding.into_completion().map_err(|_| Failure::Binding)?
            }
            PhyTemperatureExternalBinding::Sample(_) => {
                let mut job = Job::code(timer.now())?;
                drive(&mut job, registers, timer, timing.as_deref_mut()).await?;
                let value = job.code_value()?;
                PhyTemperatureCompletion::CodeSampled { value }
            }
        };
        transition
            .advance(completion)
            .map_err(|_| Failure::Binding)?;
    }
}
