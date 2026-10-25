use super::*;
use crate::analog::i2c::analog_i2c;
use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cOutcome};

#[test]
fn i2c_binding_preserves_read_identity_through_completion() {
    let action = PhyTemperatureTransition::new().action();
    let mut binding = PhyTemperatureI2cBinding::new(action).unwrap();
    assert_eq!(
        binding.action(),
        PhyColdI2cAction::StartRead {
            address: analog_i2c::saradc::i2c_saradc_tsens_dac::I2C_SARADC_TSENS_DAC.address(),
        }
    );
    binding.read_started().unwrap();
    assert_eq!(
        binding.observe_read_result(Ok(15)).unwrap(),
        PhyColdI2cObservation::EdgeConsumed
    );
    assert_eq!(
        binding.action(),
        PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
            address: analog_i2c::saradc::i2c_saradc_tsens_dac::I2C_SARADC_TSENS_DAC.address(),
            value: 15,
        })
    );
    assert_eq!(
        binding.into_completion().unwrap(),
        PhyTemperatureCompletion::MaskedRead {
            field: analog_i2c::saradc::i2c_saradc_tsens_dac::I2C_SARADC_TSENS_DAC,
            value: 15,
        }
    );
}

#[test]
fn code_sample_binding_rejects_terminal_and_i2c_actions() {
    assert!(PhyTemperatureSampleBinding::new(PhyTemperatureAction::SampleCode).is_ok());
    assert_eq!(
        PhyTemperatureSampleBinding::new(PhyTemperatureTransition::new().action()),
        Err(PhyTemperatureBindingError::UnsupportedAction)
    );
}

#[test]
fn external_lowering_covers_temperature_i2c_and_sample_actions() {
    assert!(matches!(
        PhyTemperatureExternalBinding::lower(PhyTemperatureTransition::new().action()),
        Ok(PhyTemperatureExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyTemperatureExternalBinding::lower(PhyTemperatureAction::SampleCode),
        Ok(PhyTemperatureExternalBinding::Sample(_))
    ));
}
