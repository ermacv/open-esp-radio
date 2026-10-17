use super::*;
use crate::analog::i2c::PhyI2cAddress;

#[test]
fn temperature_partition_matches_all_reviewed_boundaries() {
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(-20),
        PhyWifiI2cTrackingBand::Cold
    );
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(-19),
        PhyWifiI2cTrackingBand::Nominal
    );
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(54),
        PhyWifiI2cTrackingBand::Nominal
    );
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(55),
        PhyWifiI2cTrackingBand::Elevated
    );
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(94),
        PhyWifiI2cTrackingBand::Elevated
    );
    assert_eq!(
        PhyWifiI2cTrackingBand::for_temperature(95),
        PhyWifiI2cTrackingBand::Hot
    );
}

#[test]
fn unchanged_band_completes_without_touching_i2c() {
    let transition = PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
        current_temperature: 25,
        previous_band: PhyWifiI2cTrackingBand::Nominal,
    });
    assert_eq!(
        transition.action(),
        PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
            band: PhyWifiI2cTrackingBand::Nominal,
            changed: false,
        })
    );
}

#[test]
fn hot_transition_owns_both_masked_writes_before_commit() {
    let mut transition = PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
        current_temperature: 95,
        previous_band: PhyWifiI2cTrackingBand::Nominal,
    });
    complete_write(
        &mut transition,
        analog_registers::WIFI_TX_TEMPERATURE_TRACKING_0.address(),
        0xa0,
        0xa6,
    );
    complete_write(
        &mut transition,
        analog_registers::WIFI_TX_TEMPERATURE_TRACKING_1.address(),
        0x40,
        0x4f,
    );
    assert_eq!(
        transition.action(),
        PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
            band: PhyWifiI2cTrackingBand::Hot,
            changed: true,
        })
    );
}

fn complete_write(
    transition: &mut PhyWifiI2cTrackingTransition,
    address: PhyI2cAddress,
    read_value: u8,
    written_value: u8,
) {
    assert_eq!(
        transition.action(),
        PhyWifiI2cTrackingAction::MaskedWrite(oer_radio_analog::Action::Read { address })
    );
    transition
        .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(
            oer_radio_analog::Completion::Read {
                address,
                value: read_value,
            },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyWifiI2cTrackingAction::MaskedWrite(oer_radio_analog::Action::Write {
            address,
            value: written_value,
        })
    );
    transition
        .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(
            oer_radio_analog::Completion::Written { address },
        ))
        .unwrap();
}
