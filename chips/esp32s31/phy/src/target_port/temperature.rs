//! One temperature-sensor transition through the production executors.
//!
//! The entry requires an already exclusive PHY borrow. It reads the sensor DAC,
//! primes the default DAC after reset, samples one code and writes the next DAC
//! range when the sample leaves the current one, exactly as the transition
//! embedded in channel selection, calibration and tracking does. It publishes
//! no temperature observation; callers own that state.

use oer_esp32s31_hal::owner::SharedPhyAccess;

use crate::{
    analog::temperature::{
        PhyTemperatureAction, PhyTemperatureExternalBinding, PhyTemperatureFailure,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
    executor::wait::{PhyShortDelay, delay},
    target_executor::{PhyTargetPortError, complete_temperature_i2c},
};

/// Run one complete temperature transition. The outer error is an executor
/// failure; the inner one is the transition's own fail-closed outcome.
pub async fn sample<D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    registers: &mut impl SharedPhyAccess,
) -> Result<Result<PhyTemperatureOutcome, PhyTemperatureFailure>, PhyTargetPortError> {
    let mut transition = PhyTemperatureTransition::new();
    loop {
        let action = transition.action();
        match action {
            PhyTemperatureAction::Complete(outcome) => return Ok(Ok(outcome)),
            PhyTemperatureAction::Failed(failure) => return Ok(Err(failure)),
            _ => {}
        }
        let completion = match PhyTemperatureExternalBinding::lower(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
        {
            PhyTemperatureExternalBinding::I2c(binding) => {
                complete_temperature_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(timer, kind, micros)
                })
                .await?
            }
            PhyTemperatureExternalBinding::Sample(binding) => binding.execute_target(registers),
        };
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}
