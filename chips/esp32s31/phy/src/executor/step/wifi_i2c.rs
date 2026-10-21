//! T3 retains each masked read/write until its P1 transport job completes.
use super::{Failure, Job, Registers, drive};
use crate::tracking::i2c::{
    PhyWifiI2cTrackingAction, PhyWifiI2cTrackingCompletion, PhyWifiI2cTrackingOutcome,
    PhyWifiI2cTrackingTransition,
};
use crate::tracking::observation::EngineTiming;
use oer_time::Timer;

pub(crate) async fn track<D: crate::executor::wait::PhyShortDelay, R: Registers, T: Timer>(
    transition: &mut PhyWifiI2cTrackingTransition,
    registers: &mut R,
    timer: &T,
    mut timing: Option<&mut EngineTiming>,
) -> Result<PhyWifiI2cTrackingOutcome, Failure> {
    loop {
        if let PhyWifiI2cTrackingAction::Complete(outcome) = transition.action() {
            return Ok(outcome);
        }
        let mut binding = transition.lower_external().map_err(|_| Failure::Binding)?;
        {
            let mut job = Job::counted_i2c(binding.transaction_mut(), timer.now())?;
            drive::<D, _, _>(
                &mut job,
                registers,
                timer,
                timing.as_deref_mut(),
                None,
                None,
            )
            .await?;
        }
        let completion = binding.into_completion().map_err(|_| Failure::Binding)?;
        transition
            .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(completion))
            .map_err(|_| Failure::Binding)?;
    }
}
