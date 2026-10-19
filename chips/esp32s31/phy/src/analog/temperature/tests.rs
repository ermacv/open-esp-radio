use crate::analog::i2c::analog_registers;

use super::{
    PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureFailure, PhyTemperatureOutcome,
    PhyTemperatureTransition, PhyTemperatureTransitionError, temperature_from_code,
};

fn complete_dac_read(transition: &mut PhyTemperatureTransition, dac: u8) {
    transition
        .advance(PhyTemperatureCompletion::MaskedRead {
            field: analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
            value: dac,
        })
        .unwrap();
}

#[test]
fn all_five_recovered_dac_codes_are_accepted() {
    for dac in [5, 7, 15, 11, 10] {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, dac);
        assert_eq!(transition.action(), PhyTemperatureAction::SampleCode);
    }
}

#[test]
fn conversion_and_in_range_path_match_rom_integer_arithmetic() {
    assert_eq!(temperature_from_code(128, -2), 91);
    let mut transition = PhyTemperatureTransition::new();
    complete_dac_read(&mut transition, 5);
    transition
        .advance(PhyTemperatureCompletion::CodeSampled { value: 128 })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::Complete(PhyTemperatureOutcome {
            temperature: 91,
            sensor_index: 0,
            next_dac: 5,
        })
    );
}

#[test]
fn saturated_range_waits_for_reapplication_even_when_dac_is_unchanged() {
    for (dac, code) in [(5, 255), (10, 0)] {
        let mut transition = PhyTemperatureTransition::new();
        complete_dac_read(&mut transition, dac);
        transition
            .advance(PhyTemperatureCompletion::CodeSampled { value: code })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTemperatureAction::WriteMasked {
                field: analog_registers::TEMPERATURE_SENSOR_DAC,
                value: dac,
            }
        );
        assert_eq!(
            transition.advance(PhyTemperatureCompletion::MaskedWrite {
                field: analog_registers::TEMPERATURE_SENSOR_DAC,
                value: dac ^ 1,
            }),
            Err(PhyTemperatureTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyTemperatureCompletion::MaskedWrite {
                field: analog_registers::TEMPERATURE_SENSOR_DAC,
                value: dac,
            })
            .unwrap();
        assert!(
            matches!(transition.action(), PhyTemperatureAction::Complete(outcome)
            if outcome.next_dac == dac)
        );
    }
}

#[test]
fn range_change_requires_an_exact_i2c_write_completion() {
    let mut transition = PhyTemperatureTransition::new();
    complete_dac_read(&mut transition, 15);
    transition
        .advance(PhyTemperatureCompletion::CodeSampled { value: 255 })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::WriteMasked {
            field: analog_registers::TEMPERATURE_SENSOR_DAC,
            value: 7,
        }
    );
    assert_eq!(
        transition.advance(PhyTemperatureCompletion::MaskedWrite {
            field: analog_registers::TEMPERATURE_SENSOR_DAC,
            value: 5,
        }),
        Err(PhyTemperatureTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyTemperatureCompletion::MaskedWrite {
            field: analog_registers::TEMPERATURE_SENSOR_DAC,
            value: 7,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::Complete(PhyTemperatureOutcome {
            temperature: 91,
            sensor_index: 2,
            next_dac: 7,
        })
    );
}

#[test]
fn reset_dac_is_primed_to_the_first_rom_range_before_sampling() {
    let mut transition = PhyTemperatureTransition::new();
    complete_dac_read(&mut transition, 0);
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::WriteMasked {
            field: analog_registers::TEMPERATURE_SENSOR_DAC,
            value: 5,
        }
    );
    transition
        .advance(PhyTemperatureCompletion::MaskedWrite {
            field: analog_registers::TEMPERATURE_SENSOR_DAC,
            value: 5,
        })
        .unwrap();
    assert_eq!(transition.action(), PhyTemperatureAction::SampleCode);
    // The primed sample uses the first ROM window: DAC 5, calibration -2.
    transition
        .advance(PhyTemperatureCompletion::CodeSampled { value: 128 })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::Complete(PhyTemperatureOutcome {
            temperature: 91,
            sensor_index: 0,
            next_dac: 5,
        })
    );
}

#[test]
fn non_reset_invalid_dac_is_a_typed_failure() {
    let mut transition = PhyTemperatureTransition::new();
    complete_dac_read(&mut transition, 1);
    assert_eq!(
        transition.action(),
        PhyTemperatureAction::Failed(PhyTemperatureFailure::InvalidDac(1))
    );
}

#[test]
fn sample_step_rejects_a_non_sample_completion() {
    let mut transition = PhyTemperatureTransition::new();
    complete_dac_read(&mut transition, 5);
    assert_eq!(
        transition.advance(PhyTemperatureCompletion::MaskedRead {
            field: analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
            value: 128,
        }),
        Err(PhyTemperatureTransitionError::WrongCompletion)
    );
}
