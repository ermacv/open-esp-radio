//! Ownership-bound frequency-memory and channel-switch leaves.

#![forbid(unsafe_code)]

use oer_mmio::Placement;

use crate::{
    RadioPhyRegisters, generated::phy_frequency_channel_oracle::PhyFrequencyChannelOracle,
};

/// Corrected capacitor word, retaining the ROM's signed spill into the mode.
pub const fn frequency_cap_correction_word(raw: u32, adjusted: u16) -> u32 {
    crate::generated::PhyFrequencyCapCorrectionInput::compose(
        raw,
        adjusted as u8,
        adjusted as i16 as i32 as u32,
    )
    .get()
}

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

/// `phy_freq_i2c_num_addr`'s writes: the two prefix addresses in one
/// fresh-read RMW of `I2C_NUMBER_CONTROL`, then the three address words in
/// ascending order, each written whole.
fn publish_i2c_number_addresses<P: Placement>(
    frequency: &mut PhyFrequencyChannelOracle<P>,
    addresses: &PhyFrequencyI2cNumberAddresses,
) {
    let address = |index: usize| -> oer_mmio::Bits<5> {
        oer_mmio::Bits::new(u32::from(addresses.values[index]))
            .expect("validated PHY-I2C address fits five bits")
    };
    let zero = oer_mmio::Bits::<5>::constant::<0>();
    // The two prefix addresses in one fresh-read RMW.
    frequency.modify_i2c_number_control(|w| {
        w.set_bits_8_12_opaque(address(0))
            .set_bits_13_17_opaque(address(1));
    });
    frequency.frequency_i2c_number_word(
        oer_mmio::Index::constant::<0>(),
        address(2),
        address(3),
        address(4),
        address(5),
        address(6),
        address(7),
        oer_mmio::Bits::constant::<0>(),
    );
    frequency.frequency_i2c_number_word(
        oer_mmio::Index::constant::<1>(),
        address(8),
        address(9),
        address(10),
        zero,
        zero,
        zero,
        oer_mmio::Bits::constant::<0>(),
    );
    frequency.frequency_i2c_number_word(
        oer_mmio::Index::constant::<2>(),
        zero,
        zero,
        zero,
        zero,
        zero,
        zero,
        oer_mmio::Bits::constant::<0>(),
    );
}

/// Complete rev0 ROM `phy_nrx_freq_set`: two reads of `NRX_FREQUENCY_CONTROL`,
/// then one store of the quotient with the high fields of the second read.
fn publish_nrx_frequency<P: Placement>(
    frequency_registers: &mut PhyFrequencyChannelOracle<P>,
    frequency: u32,
) {
    // The ROM intentionally samples this word twice. The first sample
    // supplies the complete eight-bit shift selector; the second supplies
    // both high fields preserved by the final publication.
    let first = frequency_registers.read_nrx_frequency_control();
    let shift = first.bits_24_28_opaque().get() + first.bits_29_31_opaque().get() * 0x20;
    let previous = frequency_registers.read_nrx_frequency_control();
    frequency_registers.publish_nrx_frequency_fields(
        oer_mmio::Bits::new(nrx_frequency_quotient(shift, frequency))
            .expect("the quotient is reduced below 2^24"),
        previous.bits_24_28_opaque(),
        previous.bits_29_31_opaque(),
    );
}

impl RadioPhyRegisters {
    /// Select the recovered two-bit baseband mode.
    pub fn set_frequency_baseband_mode(&mut self, mode: u8) {
        let mode = crate::generated::PhyFrequencyBasebandMode::new(u32::from(mode))
            .expect("baseband mode must fit its generated two-bit domain");
        self.peripherals
            .phy_frequency_channel_oracle
            .configure_phy_frequency_baseband_mode(mode);
    }

    /// Pulse the frequency-module reset/release bit through two fresh RMWs.
    pub fn reset_frequency_module(&mut self) {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        frequency.assert_phy_frequency_module_reset();
        frequency.release_phy_frequency_module_reset();
    }

    /// Retain the two ordered observations after software frequency ownership settles.
    pub fn observe_software_frequency_boundary(&mut self) {
        let _ = self.sample_sdm_deadline_counter();
        let _ = self
            .peripherals
            .phy_frequency_channel_oracle
            .read_i2c_number_control();
    }

    /// Select whether hardware owns frequency updates.
    pub fn set_hardware_frequency_control(&mut self, enabled: bool) {
        let state = if enabled {
            crate::generated::PhyHardwareFrequencyControlState::Hardware
        } else {
            crate::generated::PhyHardwareFrequencyControlState::Software
        };
        self.peripherals
            .phy_frequency_channel_oracle
            .configure_phy_hardware_frequency_control(state);
    }

    /// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
    pub fn initialize_frequency_registers(&mut self, parameter_override: bool) {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        frequency.initialize_phy_frequency_control();
        frequency.enable_phy_frequency_module();
        let mode = if parameter_override {
            crate::generated::PhyFrequencyRegisterMode::ParameterOverride
        } else {
            crate::generated::PhyFrequencyRegisterMode::Default
        };
        frequency.configure_phy_frequency_register_mode(mode);
        // Retain the complete vendor initialization image as one operation.
        // Its unknown-reset lanes have no independent field capabilities;
        // callers cannot supply a different word.
        frequency.frequency_parameter_0();
        // The second whole-word image has the same closed contract.
        // Individual fields do not grant access to its initialization padding.
        frequency.frequency_parameter_1_initialization();
    }

    /// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
    pub fn write_frequency_memory(&mut self, address: u16, value: u32, mode: u8) {
        assert!(
            value <= 0x00ff_ffff,
            "frequency-memory data must fit 24 bits"
        );
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        let address = crate::generated::PhyFrequencyMemoryAddress::new(u32::from(address))
            .expect("frequency-memory address must fit its generated eleven-bit domain");
        frequency.configure_phy_frequency_memory_address(address);
        frequency.frequency_memory_data(
            oer_mmio::Bits::new(value).expect("frequency-memory data fits 24 bits"),
            mode,
        );
        frequency.raise_phy_frequency_memory_write_pulse();
        frequency.lower_phy_frequency_memory_write_pulse();
    }

    /// Apply complete rev0 ROM `phy_read_rf_freq_mem`.
    pub fn read_frequency_memory(&mut self, address: u16, mode: u8) -> u32 {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        let address = crate::generated::PhyFrequencyMemoryAddress::new(u32::from(address))
            .expect("frequency-memory address must fit its generated eleven-bit domain");
        let mode = crate::generated::PhyFrequencyMemoryReadMode::new(u32::from(mode))
            .expect("frequency-memory read mode must fit its generated two-bit domain");
        frequency.configure_phy_frequency_memory_address(address);
        frequency.configure_phy_frequency_memory_read_mode(mode);
        frequency.raise_phy_frequency_memory_read_pulse();
        frequency.lower_phy_frequency_memory_read_pulse();
        frequency.read_frequency_memory_read_result().value()
    }

    /// Restore the low frequency-control channel index without pulsing a switch.
    pub fn set_frequency_channel_index(&mut self, frequency_index: u8) {
        let index = crate::generated::PhyFrequencyChannelIndex::new(u32::from(frequency_index))
            .expect("one byte fits the complete generated channel-index domain");
        self.peripherals
            .phy_frequency_channel_oracle
            .configure_phy_frequency_channel_index(index);
    }

    /// Publish the complete semantic `phy_freq_i2c_num_addr` sequence.
    pub fn configure_frequency_i2c_number_addresses(
        &mut self,
        addresses: PhyFrequencyI2cNumberAddresses,
    ) {
        publish_i2c_number_addresses(
            &mut self.peripherals.phy_frequency_channel_oracle,
            &addresses,
        );
    }

    /// Publish the two pre-delay edges of `phy_freq_chan_en_sw`.
    pub fn start_frequency_channel_switch(&mut self, frequency_index: u8) {
        self.set_frequency_channel_index(frequency_index);
        self.peripherals
            .phy_frequency_channel_oracle
            .raise_phy_frequency_channel_switch_pulse();
    }

    /// Complete the caller-delayed clear of `phy_freq_chan_en_sw`.
    pub fn clear_frequency_channel_switch(&mut self) {
        self.peripherals
            .phy_frequency_channel_oracle
            .lower_phy_frequency_channel_switch_pulse();
    }

    /// Sample the generated frequency-ready field exactly once.
    pub fn frequency_ready(&mut self) -> bool {
        self.peripherals
            .phy_frequency_channel_oracle
            .read_frequency_parameter_1_status()
            .frequency_ready()
    }

    /// Apply complete rev0 ROM `phy_nrx_freq_set`.
    pub fn configure_nrx_frequency(&mut self, frequency: u32) {
        publish_nrx_frequency(
            &mut self.peripherals.phy_frequency_channel_oracle,
            frequency,
        );
    }

    /// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_nrx_baseband(&mut self) {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        // The complete ROM body replaces the generated twenty-four-bit
        // multifunction field with this bounded initialization image.
        frequency.initialize_phy_nrx_frequency_quotient();
        frequency.initialize_phy_nrx_frequency_shift();
    }

    /// Publish the channel-offset prefix of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_prefix(&mut self, cbw: u8) {
        let value = bss_tx_offset(cbw);
        let value = crate::generated::PhyChannelOffsetNibble::new(u32::from(value))
            .expect("reviewed BSS-CBW offset fits its generated nibble domain");
        self.peripherals
            .phy_frequency_channel_oracle
            .configure_phy_bss_channel_offset(value);
    }

    /// Publish the three FBW suffix edges of complete `phy_bb_bss_cbw40`.
    pub fn configure_bss_cbw_suffix(&mut self, cbw: u8) {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        let selection = if cbw == 0 {
            crate::generated::PhyFbwSelectionState::Unselected
        } else {
            crate::generated::PhyFbwSelectionState::Selected
        };
        frequency.clear_phy_fbw_control();
        frequency.configure_phy_fbw_select_mid(selection);
        frequency.configure_phy_fbw_select_high(selection);
    }

    /// Apply the four fresh-read replacements of complete `phy_bb_cbw_chan_cfg`.
    pub fn configure_channel_cbw(&mut self, cbw: u32) {
        let fields = channel_cbw_fields(cbw);
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
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
        frequency.configure_phy_channel_cbw_offset(tx_offset);
        frequency.configure_phy_channel_cbw_control_0(control_0);
        frequency.configure_phy_channel_cbw_control_1_high(control_1_high);
        frequency.configure_phy_channel_cbw_control_1_low(control_1_low);
    }

    /// Apply the three fresh-read updates of complete `phy_bt_filter_reg`.
    pub fn configure_bt_filter(&mut self) {
        let frequency = &mut self.peripherals.phy_frequency_channel_oracle;
        frequency.enable_phy_bt_filter();
        frequency.clear_phy_bt_filter_low();
        frequency.clear_phy_bt_filter_mode();
    }

    /// Publish the TX-cap readback into command-memory entry one.
    pub fn publish_frequency_tx_cap(&mut self, value: u8) {
        self.peripherals.phy_i2c_command_ram.phy_i2c_command_memory(
            oer_mmio::Index::constant::<1>(),
            crate::generated::phy_i2c_command_ram::command_memory::Block::Txrf,
            2,
            value,
        );
    }
}

#[cfg(test)]
mod tests;
