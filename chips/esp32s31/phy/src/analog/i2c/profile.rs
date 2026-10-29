//! S31 retained-wake profile. SOURCE: BLOB_LIBPHY_PHY_I2C_INIT2_20260907.
use super::{Address, PhyRfInitParameterSnapshot, analog_i2c};
use oer_espressif_phy::i2c::PhyI2cParallelWrite;

// SOURCE: BLOB_LIBPHY_PHY_SET_RATE_CLASS; opaque-blobray-phy.md, TXRF.
// Existing vendor initialization 0x08 selects class 1 with all other bits 0.
const TXRF_INITIAL_RATE_CLASS: u8 = match analog_i2c::txrf::byte_0a_opaque::PHY_SET_RATE_CLASS
    .insert(
        0,
        analog_i2c::txrf::byte_0a_opaque::PHY_SET_RATE_CLASS_ARGUMENT_AT_LEAST_8,
    ) {
    Some(value) => value,
    None => panic!("the named initial rate class fits the model field"),
};

const PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT: usize = 22;

impl PhyRfInitParameterSnapshot {
    /// Return the retained-wake stage-two pair at `index`, or `None` after the last.
    pub(crate) fn initialization_stage_two_pair(
        self,
        index: usize,
    ) -> Option<PhyI2cParallelWrite<Address>> {
        if index >= PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT {
            return None;
        }

        const FIRST_ADDRESSES: [Address; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            analog_i2c::txrf::tx_capacitor_low::ADDRESS,
            analog_i2c::txrf::wifi_tx_temperature_tracking_0::ADDRESS,
            analog_i2c::txrf::byte_04_opaque::ADDRESS,
            analog_i2c::txrf::byte_0e_opaque::ADDRESS,
            analog_i2c::txrf::byte_09_opaque::ADDRESS,
            analog_i2c::txrf::wifi_tx_temperature_tracking_1::ADDRESS,
            analog_i2c::txrf::byte_08_opaque::ADDRESS,
            analog_i2c::txrf::byte_05_opaque::ADDRESS,
            analog_i2c::txrf::byte_06_opaque::ADDRESS,
            analog_i2c::txrf::byte_0c_opaque::ADDRESS,
            analog_i2c::txrf::byte_0d_opaque::ADDRESS,
            analog_i2c::txrf::byte_0a_opaque::ADDRESS,
            analog_i2c::txrf::byte_0b_opaque::ADDRESS,
            analog_i2c::txrf::rc_configuration_1::ADDRESS,
            analog_i2c::txrf::byte_01_opaque::ADDRESS,
            analog_i2c::rfpll::restart_high::ADDRESS,
            analog_i2c::rfpll::dcode_ckgen_reset::ADDRESS,
            analog_i2c::rfpll::charge_pump_calibration_enable::ADDRESS,
            analog_i2c::rfpll::capacitor_search_enable::ADDRESS,
            analog_i2c::rfpll::byte_0d_opaque::ADDRESS,
            analog_i2c::rfpll::byte_15_opaque::ADDRESS,
            analog_i2c::bbpll::oc_ref_div::ADDRESS,
        ];
        const FIRST_FIXED_VALUES: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x73,
            0xba,
            0x88,
            0xf4,
            0x02,
            0xfd,
            0xbf,
            0x01,
            0x11,
            0xa7,
            0x77,
            TXRF_INITIAL_RATE_CLASS,
            0x04,
            0x81,
            0x01,
            0x68,
            0xa8,
            0,
            0x44,
            0x0a,
            0x08,
            0x70,
        ];
        const SECOND_ADDRESSES: [Address; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            analog_i2c::bbtop::rf_close_control::ADDRESS,
            analog_i2c::bbtop::byte_14_opaque::ADDRESS,
            analog_i2c::bbtop::byte_15_opaque::ADDRESS,
            analog_i2c::bbtop::byte_16_opaque::ADDRESS,
            analog_i2c::bbtop::byte_17_opaque::ADDRESS,
            analog_i2c::bbtop::byte_18_opaque::ADDRESS,
            analog_i2c::bbtop::byte_19_opaque::ADDRESS,
            analog_i2c::bbtop::byte_1c_opaque::ADDRESS,
            analog_i2c::bbtop::byte_1d_opaque::ADDRESS,
            analog_i2c::bbtop::bluetooth_tx_power_high_0::ADDRESS,
            analog_i2c::bbtop::bluetooth_tx_power_high_1::ADDRESS,
            analog_i2c::bbtop::byte_04_opaque::ADDRESS,
            analog_i2c::bbtop::byte_05_opaque::ADDRESS,
            analog_i2c::bbtop::byte_06_opaque::ADDRESS,
            analog_i2c::bbtop::byte_07_opaque::ADDRESS,
            analog_i2c::bbtop::byte_0c_opaque::ADDRESS,
            analog_i2c::bbtop::byte_0d_opaque::ADDRESS,
            analog_i2c::bbtop::byte_0e_opaque::ADDRESS,
            analog_i2c::bbtop::byte_0f_opaque::ADDRESS,
            analog_i2c::sdm::low::ADDRESS,
            analog_i2c::sdm::low::ADDRESS,
            analog_i2c::sdm::low::ADDRESS,
        ];

        let high_filter = saturate_phy_i2c_value(
            i32::from(self.filter_dcap.parameter_ed).saturating_add(6),
            0x3c,
            2,
        );
        let low_filter = saturate_phy_i2c_value(
            i32::from(self.filter_dcap.parameter_ed).saturating_sub(2),
            0x3c,
            2,
        );
        let auxiliary = self.filter_dcap.parameter_ee.wrapping_add(2);
        let second_value = match index {
            0 => 0x27,
            1 | 2 => high_filter,
            3 => low_filter,
            4 => self.filter_dcap.parameter_ed,
            5 | 6 => auxiliary,
            7 | 8 | 10 => self.filter_dcap.parameter_f0,
            9 => analog_i2c::bbtop::bluetooth_tx_power_high_0::BIT_6_OPAQUE
                .insert_truncated(self.filter_dcap.parameter_f0, 1),
            11 | 12 | 15 | 16 => self.filter_dcap.parameter_e9,
            13 | 14 | 17 | 18 => self.filter_dcap.parameter_ea,
            19..=21 => 0,
            _ => return None,
        };
        let first_value = if index == 17 {
            self.parameter_18e()
        } else {
            *FIRST_FIXED_VALUES.get(index)?
        };

        Some(PhyI2cParallelWrite {
            first: (*FIRST_ADDRESSES.get(index)?, first_value),
            second: (*SECOND_ADDRESSES.get(index)?, second_value),
        })
    }
}

const fn saturate_phy_i2c_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analog::i2c::FilterDcapParameters;
    #[test]
    fn current_vendor_profile_keeps_pair_count_dynamic_values_and_initial_rate_class() {
        let profile = PhyRfInitParameterSnapshot::new(
            FilterDcapParameters::new(0x22, 0x33, 0x3e, 0x55, 0x66),
            0x11,
        );
        assert_eq!(
            profile.initialization_stage_two_pair(0),
            Some(PhyI2cParallelWrite {
                first: (analog_i2c::txrf::tx_capacitor_low::ADDRESS, 0x73),
                second: (analog_i2c::bbtop::rf_close_control::ADDRESS, 0x27)
            })
        );
        assert_eq!(
            profile.initialization_stage_two_pair(11).unwrap().first,
            (analog_i2c::txrf::byte_0a_opaque::ADDRESS, 0x08)
        );
        assert_eq!(
            profile.initialization_stage_two_pair(17).unwrap().first.1,
            0x11
        );
        assert_eq!(
            profile.initialization_stage_two_pair(1).unwrap().second.1,
            0x3c
        );
        assert!(profile.initialization_stage_two_pair(21).is_some());
        assert_eq!(profile.initialization_stage_two_pair(22), None);
        assert_eq!(profile.initialization_stage_two_pair(usize::MAX), None);
    }
}
