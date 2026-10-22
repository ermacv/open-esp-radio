//! Shared cold/runtime D-code edges. The model owns channel and capture order.
use super::{Failure, Job, Registers, drive};
use crate::{
    analog::{
        dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding},
        rfpll::RfpllFrequencyExternalBinding,
    },
    tracking::observation::EngineTiming,
};
use oer_time::Timer;

pub(crate) async fn complete<D: crate::executor::wait::PhyShortDelay, R: Registers, T: Timer>(
    binding: PhyDcodeExternalBinding,
    registers: &mut R,
    timer: &T,
    runtime: bool,
    timing: Option<&mut EngineTiming>,
) -> Result<PhyDcodeCompletion, Failure> {
    match binding {
        PhyDcodeExternalBinding::I2c(mut binding) => {
            let mut job = if runtime {
                Job::direct_i2c(binding.transaction_mut(), timer.now())?
            } else {
                Job::counted_i2c(binding.transaction_mut(), timer.now())?
            };
            drive::<D, _, _>(&mut job, registers, timer, timing, None, None).await?;
            binding.into_completion().map_err(|_| Failure::Binding)
        }
        PhyDcodeExternalBinding::Rfpll(RfpllFrequencyExternalBinding::I2c(mut binding))
            if !runtime =>
        {
            let mut job = Job::counted_i2c(binding.transaction_mut(), timer.now())?;
            drive::<D, _, _>(&mut job, registers, timer, timing, None, None).await?;
            binding
                .into_completion()
                .map(PhyDcodeCompletion::Rfpll)
                .map_err(|_| Failure::Binding)
        }
        PhyDcodeExternalBinding::Rfpll(binding) => {
            let mut binding = Some(binding);
            let mut completion = None;
            let mut job = Job::frequency(&mut binding, &mut completion, timer.now())?;
            drive::<D, _, _>(&mut job, registers, timer, timing, None, None).await?;
            completion
                .map(PhyDcodeCompletion::Rfpll)
                .ok_or(Failure::Binding)
        }
    }
}
