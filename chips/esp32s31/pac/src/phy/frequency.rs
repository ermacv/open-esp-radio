//! Ownership-bound frequency-memory and channel-switch leaves.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;

/// Semantic PHY-I2C number-address sequence consumed by
/// `phy_freq_i2c_num_addr`.
///
/// The register split and field placement remain private to the PAC. Callers
/// provide only the eleven addresses recovered from the vendor descriptor
/// graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cNumberAddresses {
    values: [u8; 11],
}

impl PhyFrequencyI2cNumberAddresses {
    /// Bind the complete recovered address sequence to its five-bit hardware
    /// domain.
    pub const fn new(values: [u8; 11]) -> Option<Self> {
        let mut index = 0;
        while index != values.len() {
            if values[index] > 0x1f {
                return None;
            }
            index += 1;
        }
        Some(Self { values })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChannelCbwFields {
    tx_offset: u8,
    control_0: u8,
    control_1_high: u8,
    control_1_low: u8,
}

const fn bss_tx_offset(cbw: u8) -> u8 {
    if cbw == 2 {
        2
    } else if cbw == 3 {
        1
    } else {
        0
    }
}

const fn channel_cbw_fields(cbw: u32) -> ChannelCbwFields {
    let high = cbw / 16;
    if high != 0 {
        let normalized = high.wrapping_sub(1) as u8;
        let low = (cbw % 16) as u8;
        ChannelCbwFields {
            tx_offset: low,
            control_0: low % 4,
            control_1_high: normalized % 8,
            control_1_low: normalized % 4,
        }
    } else {
        let low = (cbw % 16) as u8;
        let normalized = low.saturating_sub(2);
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized % 4,
            control_1_high: if low != 0 { 1 } else { 0 },
            control_1_low: if low >= 2 { 1 } else { 0 },
        }
    }
}

const fn rv32_signed_div(dividend: i32, divisor: i32) -> i32 {
    if divisor == 0 {
        -1
    } else if dividend == i32::MIN && divisor == -1 {
        i32::MIN
    } else {
        dividend / divisor
    }
}

fn nrx_frequency_quotient(shift: u32, frequency: u32) -> u32 {
    let numerator = 0x50_u32.wrapping_mul(2_u32.wrapping_pow(shift % 32)) as i32;
    let quotient = rv32_signed_div(numerator, frequency as i32) as u32;
    quotient % 0x0100_0000
}

impl RadioPhyRegisters {
    /// Select the recovered two-bit baseband mode.
    pub fn set_frequency_baseband_mode(&mut self, mode: u8) {
        let mode = crate::generated::PhyFrequencyBasebandMode::new(u32::from(mode))
            .expect("baseband mode must fit its generated two-bit domain");
        crate::generated::configure_phy_frequency_baseband_mode(
            &self.peripherals.phy_frequency_channel_oracle,
            mode,
        );
    }

    /// Pulse the frequency-module reset/release bit through two fresh RMWs.
    pub fn reset_frequency_module(&mut self) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        crate::generated::assert_phy_frequency_module_reset(frequency);
        crate::generated::release_phy_frequency_module_reset(frequency);
    }

    /// Retain the two ordered observations after software frequency ownership settles.
    pub fn observe_software_frequency_boundary(&mut self) {
        let _ = self.sample_sdm_deadline_counter();
        let _ = self
            .peripherals
            .phy_frequency_channel_oracle
            .i2c_number_control()
            .read();
    }

    /// Select whether hardware owns frequency updates.
    pub fn set_hardware_frequency_control(&mut self, enabled: bool) {
        let state = if enabled {
            crate::generated::PhyHardwareFrequencyControlState::Hardware
        } else {
            crate::generated::PhyHardwareFrequencyControlState::Software
        };
        crate::generated::configure_phy_hardware_frequency_control(
            &self.peripherals.phy_frequency_channel_oracle,
            state,
        );
    }

    /// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
    pub fn initialize_frequency_registers(&mut self, parameter_override: bool) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        crate::generated::initialize_phy_frequency_control(frequency);
        crate::generated::enable_phy_frequency_module(frequency);
        let mode = if parameter_override {
            crate::generated::PhyFrequencyRegisterMode::ParameterOverride
        } else {
            crate::generated::PhyFrequencyRegisterMode::Default
        };
        crate::generated::configure_phy_frequency_register_mode(frequency, mode);
        crate::generated::frequency_parameter_0(
            frequency,
            crate::generated::FrequencyParameter0Image::new(0x1980_0249),
        );
        crate::svd::zero_based_field_write::frequency_parameter_1_initialization(
            frequency, 0, 0x16, 0x12c127,
        );
    }

    /// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
    pub fn write_frequency_memory(&mut self, address: u16, value: u32, mode: u8) {
        assert!(
            value <= 0x00ff_ffff,
            "frequency-memory data must fit 24 bits"
        );
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        let address = crate::generated::PhyFrequencyMemoryAddress::new(u32::from(address))
            .expect("frequency-memory address must fit its generated eleven-bit domain");
        crate::generated::configure_phy_frequency_memory_address(frequency, address);
        crate::svd::zero_based_field_write::frequency_memory_data(frequency, value, mode);
        crate::generated::raise_phy_frequency_memory_write_pulse(frequency);
        crate::generated::lower_phy_frequency_memory_write_pulse(frequency);
    }

    /// Apply complete rev0 ROM `phy_read_rf_freq_mem`.
    pub fn read_frequency_memory(&mut self, address: u16, mode: u8) -> u32 {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        let address = crate::generated::PhyFrequencyMemoryAddress::new(u32::from(address))
            .expect("frequency-memory address must fit its generated eleven-bit domain");
        let mode = crate::generated::PhyFrequencyMemoryReadMode::new(u32::from(mode))
            .expect("frequency-memory read mode must fit its generated two-bit domain");
        crate::generated::configure_phy_frequency_memory_address(frequency, address);
        crate::generated::configure_phy_frequency_memory_read_mode(frequency, mode);
        crate::generated::raise_phy_frequency_memory_read_pulse(frequency);
        crate::generated::lower_phy_frequency_memory_read_pulse(frequency);
        crate::svd::field_read::observe_phy_frequency_memory_result(frequency)
    }

    /// Restore the low frequency-control channel index without pulsing a switch.
    pub fn set_frequency_channel_index(&mut self, frequency_index: u8) {
        let index = crate::generated::PhyFrequencyChannelIndex::new(u32::from(frequency_index))
            .expect("one byte fits the complete generated channel-index domain");
        crate::generated::configure_phy_frequency_channel_index(
            &self.peripherals.phy_frequency_channel_oracle,
            index,
        );
    }

    /// Publish the complete semantic `phy_freq_i2c_num_addr` sequence.
    pub fn configure_frequency_i2c_number_addresses(
        &mut self,
        addresses: PhyFrequencyI2cNumberAddresses,
    ) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        let address_0 =
            crate::generated::PhyFrequencyI2cNumberAddress::new(u32::from(addresses.values[0]))
                .expect("validated PHY-I2C address fits its generated domain");
        let address_1 =
            crate::generated::PhyFrequencyI2cNumberAddress::new(u32::from(addresses.values[1]))
                .expect("validated PHY-I2C address fits its generated domain");
        crate::generated::configure_phy_frequency_i2c_number_prefix(
            frequency, address_0, address_1,
        );
        crate::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency,
            0,
            addresses.values[2],
            addresses.values[3],
            addresses.values[4],
            addresses.values[5],
            addresses.values[6],
            addresses.values[7],
            0,
        );
        crate::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency,
            1,
            addresses.values[8],
            addresses.values[9],
            addresses.values[10],
            0,
            0,
            0,
            0,
        );
        crate::svd::zero_based_field_write::frequency_i2c_number_word(
            frequency, 2, 0, 0, 0, 0, 0, 0, 0,
        );
    }

    /// Publish the two pre-delay edges of `phy_freq_chan_en_sw`.
    pub fn start_frequency_channel_switch(&mut self, frequency_index: u8) {
        self.set_frequency_channel_index(frequency_index);
        crate::generated::raise_phy_frequency_channel_switch_pulse(
            &self.peripherals.phy_frequency_channel_oracle,
        );
    }

    /// Complete the caller-delayed clear of `phy_freq_chan_en_sw`.
    pub fn clear_frequency_channel_switch(&mut self) {
        crate::generated::lower_phy_frequency_channel_switch_pulse(
            &self.peripherals.phy_frequency_channel_oracle,
        );
    }

    /// Sample the generated frequency-ready field exactly once.
    pub fn frequency_ready(&mut self) -> bool {
        crate::svd::field_read::observe_phy_frequency_ready(
            &self.peripherals.phy_frequency_channel_oracle,
        )
    }

    /// Apply complete rev0 ROM `phy_nrx_freq_set`.
    pub fn configure_nrx_frequency(&mut self, frequency: u32) {
        let frequency_registers = &self.peripherals.phy_frequency_channel_oracle;
        // The ROM intentionally samples this word twice. The first sample
        // supplies the complete eight-bit shift selector; the second supplies
        // both high fields preserved by the final publication.
        let (shift_low, shift_high) =
            crate::svd::field_snapshot_read::capture_phy_nrx_frequency_shift(frequency_registers);
        let shift = u32::from(shift_low) + u32::from(shift_high) * 0x20;
        let (previous_shift_low, previous_shift_high) =
            crate::svd::field_snapshot_read::capture_phy_nrx_frequency_shift(frequency_registers);
        crate::svd::zero_based_field_write::publish_nrx_frequency_fields(
            frequency_registers,
            nrx_frequency_quotient(shift, frequency),
            previous_shift_low,
            previous_shift_high,
        );
    }

    /// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_nrx_baseband(&mut self) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        // The complete ROM body replaces the generated twenty-four-bit
        // multifunction field with this bounded initialization image.
        crate::generated::initialize_phy_nrx_frequency_quotient(frequency);
        crate::generated::initialize_phy_nrx_frequency_shift(frequency);
    }

    /// Publish the channel-offset prefix of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_prefix(&mut self, cbw: u8) {
        let value = bss_tx_offset(cbw);
        let value = crate::generated::PhyChannelOffsetNibble::new(u32::from(value))
            .expect("reviewed BSS-CBW offset fits its generated nibble domain");
        crate::generated::configure_phy_bss_channel_offset(
            &self.peripherals.phy_frequency_channel_oracle,
            value,
        );
    }

    /// Publish the three FBW suffix edges of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_suffix(&mut self, cbw: u8) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        let selection = if cbw == 0 {
            crate::generated::PhyFbwSelectionState::Unselected
        } else {
            crate::generated::PhyFbwSelectionState::Selected
        };
        crate::generated::clear_phy_fbw_control(frequency);
        crate::generated::configure_phy_fbw_select_mid(frequency, selection);
        crate::generated::configure_phy_fbw_select_high(frequency, selection);
    }

    /// Apply the four fresh-read replacements of complete `phy_bb_cbw_chan_cfg`.
    pub fn configure_channel_cbw(&mut self, cbw: u32) {
        let fields = channel_cbw_fields(cbw);
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        // Channel offset owns only the low nibble. Preserve the adjacent
        // minimum-power setting, as both ROM channel setters do.
        let tx_offset = crate::generated::PhyChannelOffsetNibble::new(u32::from(fields.tx_offset))
            .expect("reviewed channel offset fits its generated nibble domain");
        let control_0 =
            crate::generated::PhyChannelCbwTwoBitImage::new(u32::from(fields.control_0))
                .expect("reviewed CBW control fits its generated two-bit domain");
        let control_1_high =
            crate::generated::PhyChannelCbwThreeBitImage::new(u32::from(fields.control_1_high))
                .expect("reviewed CBW control fits its generated three-bit domain");
        let control_1_low =
            crate::generated::PhyChannelCbwTwoBitImage::new(u32::from(fields.control_1_low))
                .expect("reviewed CBW control fits its generated two-bit domain");
        crate::generated::configure_phy_channel_cbw_offset(frequency, tx_offset);
        crate::generated::configure_phy_channel_cbw_control_0(frequency, control_0);
        crate::generated::configure_phy_channel_cbw_control_1_high(frequency, control_1_high);
        crate::generated::configure_phy_channel_cbw_control_1_low(frequency, control_1_low);
    }

    /// Apply the three fresh-read updates of complete `phy_bt_filter_reg`.
    pub fn configure_bt_filter(&mut self) {
        let frequency = &self.peripherals.phy_frequency_channel_oracle;
        crate::generated::enable_phy_bt_filter(frequency);
        crate::generated::clear_phy_bt_filter_low(frequency);
        crate::generated::clear_phy_bt_filter_mode(frequency);
    }

    /// Publish the TX-cap readback into command-memory entry one.
    pub fn publish_frequency_tx_cap(&mut self, value: u8) {
        crate::svd::zero_based_field_write::phy_i2c_command_memory(
            &self.peripherals.phy_i2c_command_ram,
            1,
            0x6b,
            2,
            value,
        );
    }
}

#[cfg(test)]
mod tests;
