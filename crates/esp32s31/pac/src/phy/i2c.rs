//! Ownership-bound access to the shared PHY analog-I²C master.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::PhyI2cBbpllCalibrationState;

pub use crate::generated::PhyI2cField;

/// One of the two reviewed analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Validated identity of one internal analog PHY-I²C block.
///
/// This is exposed only so ABI bridges can validate a vendor block argument
/// without inventing a register address outside the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cBlock {
    code: u8,
}

impl PhyI2cBlock {
    /// Convert the raw block byte accepted by the vendor ABI.
    pub const fn from_vendor_abi(code: u8) -> Option<Self> {
        if code >= 0x61 && code <= 0x6d {
            Some(Self { code })
        } else {
            None
        }
    }

    const fn recovered(code: u8) -> Self {
        assert!(code >= 0x61 && code <= 0x6d);
        Self { code }
    }

    const fn host(self) -> PhyI2cHost {
        match self.code {
            0x61 | 0x62 | 0x63 | 0x67 | 0x6a | 0x6b => PhyI2cHost::Host1,
            _ => PhyI2cHost::Host0,
        }
    }

    const fn read_mask_complement_low(self) -> u32 {
        match self.code {
            0x61 => 0x00ff_feff,
            0x62 => 0x00ff_ffdf,
            0x63 => 0x00ff_ffef,
            0x66 => 0x00ff_ff7f,
            0x67 => 0x00ff_fffb,
            0x69 => 0x00ff_f7ff,
            0x6a => 0x00ff_ffbf,
            0x6b => 0x00ff_fff7,
            0x6d => 0x00ff_7fff,
            _ => 0x00ff_ffff,
        }
    }
}

/// Validated address of one internal analog PHY-I²C byte register.
///
/// The PAC owns the block-to-host mapping and read-mask encoding. Higher
/// layers may retain this opaque identity while driving an asynchronous
/// transaction, but cannot decompose it into command-register fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cAddress {
    block: PhyI2cBlock,
    register: u8,
}

impl PhyI2cField {
    /// Return the opaque byte-register identity needed for the bus transfer.
    #[inline]
    pub const fn address(self) -> PhyI2cAddress {
        PhyI2cAddress::recovered(self.bank(), self.register())
    }

    /// The field's most and least significant bit in its byte register.
    #[inline]
    pub const fn bit_range(self) -> (u8, u8) {
        // Every reviewed field is one contiguous run of bits.
        let mask = self.replace(0, 0xff);
        (7 - mask.leading_zeros() as u8, mask.trailing_zeros() as u8)
    }
}

/// One finite analog-register command is still owned by its hardware host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cAccessError {
    Busy,
}

impl PhyI2cAddress {
    /// Registers of the analog register image: every register a reviewed
    /// `PhyI2cField` occupies, once, in the order the published register
    /// model defines (`register-image` of the PAC API policy).
    pub const IMAGE_LEN: usize = crate::generated::PHY_I2C_FIELDS_IMAGE.len();

    /// Register `index` of the analog register image; `None` past its end.
    pub const fn image(index: usize) -> Option<Self> {
        if index >= Self::IMAGE_LEN {
            return None;
        }
        let (block, register) = crate::generated::PHY_I2C_FIELDS_IMAGE[index];
        Some(Self::recovered(block, register))
    }

    const fn recovered(block: u8, register: u8) -> Self {
        Self {
            block: PhyI2cBlock::recovered(block),
            register,
        }
    }

    const fn host(self) -> PhyI2cHost {
        self.block.host()
    }
}

/// Reviewed analog-register identities and fields.
///
/// These are not CPU addresses. Their command-bus geometry is private to the
/// PAC; the names expose only semantics established by the recovered code.
pub mod analog_registers {
    use super::PhyI2cAddress;

    pub use crate::generated::phy_i2c_fields::*;

    // Whole-byte registers keep their address identity; their reviewed
    // geometry is the generated whole-byte field of the same name.
    pub const XTAL_DUTY_SEED: PhyI2cAddress =
        crate::generated::phy_i2c_fields::XTAL_DUTY_SEED.address();
    pub const XTAL_DUTY_CANDIDATE: PhyI2cAddress =
        crate::generated::phy_i2c_fields::XTAL_DUTY_CANDIDATE.address();
    pub const RFPLL_CAPACITOR_LOW: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RFPLL_CAPACITOR_LOW.address();
    pub const RFPLL_CALIBRATED_CAPACITOR_LOW: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RFPLL_CALIBRATED_CAPACITOR_LOW.address();
    pub const RFPLL_SDM_MOST_SIGNIFICANT_BYTE: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RFPLL_SDM_MOST_SIGNIFICANT_BYTE.address();
    pub const RFPLL_SDM_UPPER_MIDDLE_BYTE: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RFPLL_SDM_UPPER_MIDDLE_BYTE.address();
    pub const RFPLL_SDM_LOWER_MIDDLE_BYTE: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RFPLL_SDM_LOWER_MIDDLE_BYTE.address();
    pub const TX_CAPACITOR_BANKS: PhyI2cAddress =
        crate::generated::phy_i2c_fields::TX_CAPACITOR_BANKS.address();
    pub const RF_CLOSE_CONTROL: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RF_CLOSE_CONTROL.address();
    pub const RF_CLOSE_RETENTION_ZERO: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RF_CLOSE_RETENTION_ZERO.address();
    pub const RF_CLOSE_RETENTION_ONE: PhyI2cAddress =
        crate::generated::phy_i2c_fields::RF_CLOSE_RETENTION_ONE.address();
}

const PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT: usize = 45;

// Complete command order recovered from
// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. Values which depend on
// the explicit PHY parameter snapshot are replaced by
// `PhyI2cCommandMemoryInputs::dynamic_values` below.
const PHY_I2C_COMMAND_MEMORY_TEMPLATE: [(u8, u8, u8); PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT] = [
    (0x67, 0x02, 0x07),
    (0x6b, 0x01, 0x01),
    (0x6b, 0x02, 0x73),
    (0x6b, 0x03, 0xba),
    (0x6b, 0x04, 0x88),
    (0x6b, 0x05, 0x01),
    (0x6b, 0x06, 0x11),
    (0x6b, 0x07, 0xfd),
    (0x6b, 0x08, 0xbf),
    (0x6b, 0x09, 0x02),
    (0x6b, 0x0a, 0x08),
    (0x6b, 0x0b, 0x04),
    (0x6b, 0x0c, 0xa7),
    (0x6b, 0x0d, 0x77),
    (0x6b, 0x0e, 0xf4),
    (0x6b, 0x0f, 0x81),
    (0x62, 0x00, 0x68),
    (0x62, 0x04, 0xa8),
    (0x62, 0x0b, 0x44),
    (0x62, 0x0d, 0x0a),
    (0x62, 0x0f, 0x00),
    (0x62, 0x15, 0x08),
    (0x66, 0x02, 0x70),
    (0x67, 0x02, 0x27),
    (0x67, 0x04, 0x00),
    (0x67, 0x05, 0x00),
    (0x67, 0x06, 0x00),
    (0x67, 0x07, 0x00),
    (0x67, 0x0c, 0x00),
    (0x67, 0x0d, 0x00),
    (0x67, 0x0e, 0x00),
    (0x67, 0x0f, 0x00),
    (0x67, 0x14, 0x00),
    (0x67, 0x15, 0x00),
    (0x67, 0x16, 0x00),
    (0x67, 0x17, 0x00),
    (0x67, 0x18, 0x00),
    (0x67, 0x19, 0x00),
    (0x67, 0x1c, 0x00),
    (0x67, 0x1d, 0x00),
    (0x67, 0x1e, 0x00),
    (0x67, 0x1f, 0x00),
    (0x63, 0x06, 0x00),
    (0x6a, 0x00, 0xaf),
    (0x6a, 0x01, 0x7f),
];

const PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES: [usize; 19] = [
    20, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
];

/// Parameter facts needed to build the complete PAC-owned PHY-I²C command
/// memory. Offset-based names remain explicit because their electrical
/// meaning is not yet established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cCommandMemoryInputs {
    parameter_18e: u8,
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl PhyI2cCommandMemoryInputs {
    pub const fn new(
        parameter_18e: u8,
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_18e,
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    const fn dynamic_values(self) -> [u8; 19] {
        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        let auxiliary = self.parameter_ee.wrapping_add(2);
        [
            self.parameter_18e,
            self.parameter_e9,
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ea,
            self.parameter_e9,
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ea,
            high_filter,
            high_filter,
            low_filter,
            self.parameter_ed,
            auxiliary,
            auxiliary,
            self.parameter_f0,
            self.parameter_f0,
            analog_registers::FILTER_DCAP_HIGH_ENABLE_OPAQUE.replace(self.parameter_f0, 1),
            self.parameter_f0,
        ]
    }

    /// Return the retained-wake stage-two pair at `index`, or `None` after the last.
    pub const fn initialization_stage_two_pair(self, index: usize) -> Option<PhyI2cParallelWrite> {
        if index >= PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT {
            return None;
        }

        const FIRST_BLOCKS: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x62, 0x62, 0x62, 0x62, 0x62, 0x62, 0x66,
        ];
        const FIRST_REGISTERS: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x02, 0x03, 0x04, 0x0e, 0x09, 0x07, 0x08, 0x05, 0x06, 0x0c, 0x0d, 0x0a, 0x0b, 0x0f,
            0x01, 0x00, 0x04, 0x0f, 0x0b, 0x0d, 0x15, 0x02,
        ];
        const FIRST_FIXED_VALUES: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x73, 0xba, 0x88, 0xf4, 0x02, 0xfd, 0xbf, 0x01, 0x11, 0xa7, 0x77, 0x08, 0x04, 0x81,
            0x01, 0x68, 0xa8, 0, 0x44, 0x0a, 0x08, 0x70,
        ];
        const SECOND_BLOCKS: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67, 0x67,
            0x67, 0x67, 0x67, 0x67, 0x67, 0x63, 0x63, 0x63,
        ];
        const SECOND_REGISTERS: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x02, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1c, 0x1d, 0x1e, 0x1f, 0x04, 0x05, 0x06,
            0x07, 0x0c, 0x0d, 0x0e, 0x0f, 0x06, 0x06, 0x06,
        ];

        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        let auxiliary = self.parameter_ee.wrapping_add(2);
        let second_value = match index {
            0 => 0x27,
            1 | 2 => high_filter,
            3 => low_filter,
            4 => self.parameter_ed,
            5 | 6 => auxiliary,
            7 | 8 | 10 => self.parameter_f0,
            9 => analog_registers::FILTER_DCAP_HIGH_ENABLE_OPAQUE.replace(self.parameter_f0, 1),
            11 | 12 | 15 | 16 => self.parameter_e9,
            13 | 14 | 17 | 18 => self.parameter_ea,
            19..=21 => 0,
            _ => unreachable!(),
        };
        let first_value = if index == 17 {
            self.parameter_18e
        } else {
            FIRST_FIXED_VALUES[index]
        };

        Some(PhyI2cParallelWrite {
            first: (FIRST_BLOCKS[index], FIRST_REGISTERS[index], first_value),
            second: (SECOND_BLOCKS[index], SECOND_REGISTERS[index], second_value),
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

/// One pair of simultaneous writes on both PHY-I²C hosts.
///
/// Analog identities and values stay private to the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cParallelWrite {
    first: (u8, u8, u8),
    second: (u8, u8, u8),
}

const PHY_FILTER_DCAP_COMMAND_COUNT: u8 = 18;
const PHY_I2C_INITIALIZATION_STAGE_ONE_COMMAND_COUNT: u8 = 26;
const PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT: usize = 22;
const PHY_BIAS_REGISTER_COMMAND_COUNT: u8 = 2;
const PHY_BBPLL_CALIBRATION_COMMAND_COUNT: u8 = 2;
const PHY_RC_CALIBRATION_SETTINGS_COMMAND_COUNT: u8 = 3;
const PHY_SAR2_INITIALIZATION_COMMAND_COUNT: u8 = 2;

/// One of the two complete-ROM ADC-rate selections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyAdcRate {
    Low,
    High,
}

impl PhyAdcRate {
    /// Decode the low selector bit accepted by the complete vendor leaf.
    pub const fn from_vendor_rate(rate: u32) -> Self {
        if rate & 1 == 0 { Self::Low } else { Self::High }
    }

    const fn analog_configuration_field_value(self) -> u8 {
        match self {
            Self::Low => 2,
            Self::High => 0,
        }
    }
}

/// One step of a complete recovered PHY-I²C configuration operation.
///
/// Analog identities remain opaque; `Modify` replaces one reviewed field of
/// the value read back from its register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationCommand {
    Read(PhyI2cAddress),
    Write(PhyI2cAddress, u8),
    Modify(PhyI2cField, u8),
}

impl PhyI2cConfigurationCommand {
    /// The byte register addressed by this command.
    pub const fn address(self) -> PhyI2cAddress {
        match self {
            Self::Read(address) | Self::Write(address, _) => address,
            Self::Modify(field, _) => field.address(),
        }
    }
}

const fn bias_register_command(index: u8) -> Option<(u8, u8, u8)> {
    match index {
        0 => Some((0x6a, 0x00, 0xaf)),
        1 => Some((0x6a, 0x01, 0x7f)),
        _ => None,
    }
}

/// Parameter facts consumed by the complete PAC-owned filter-DCAP operation.
///
/// Offset-based names remain explicit because the electrical meaning of these
/// retained vendor parameters is not yet established. Analog-register
/// identities and value encodings remain private to the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFilterDcapInputs {
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl PhyFilterDcapInputs {
    pub const fn new(
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    const fn command(self, index: u8) -> Option<(u8, u8, u8)> {
        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        match index {
            0 => Some((0x67, 0x14, high_filter)),
            1 => Some((0x67, 0x15, high_filter)),
            2 => Some((0x67, 0x16, low_filter)),
            3 => Some((0x67, 0x17, self.parameter_ed)),
            4 => Some((0x67, 0x18, self.parameter_ee)),
            5 => Some((0x67, 0x19, self.parameter_ee)),
            6 => Some((0x67, 0x1c, self.parameter_f0)),
            7 => Some((0x67, 0x1d, self.parameter_f0)),
            8 => Some((
                0x67,
                0x1e,
                analog_registers::FILTER_DCAP_HIGH_ENABLE_OPAQUE.replace(self.parameter_f0, 1),
            )),
            9 => Some((0x67, 0x1f, self.parameter_f0)),
            10 => Some((0x67, 0x04, self.parameter_e9)),
            11 => Some((0x67, 0x05, self.parameter_e9)),
            12 => Some((0x67, 0x06, self.parameter_ea)),
            13 => Some((0x67, 0x07, self.parameter_ea)),
            14 => Some((0x67, 0x0c, self.parameter_e9)),
            15 => Some((0x67, 0x0d, self.parameter_e9)),
            16 => Some((0x67, 0x0e, self.parameter_ea)),
            17 => Some((0x67, 0x0f, self.parameter_ea)),
            _ => None,
        }
    }
}

/// Dynamic facts consumed by the first complete PHY-I²C initialization stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cInitializationStageOneInputs {
    parameter_18e: u8,
    parameter_ee: u8,
}

impl PhyI2cInitializationStageOneInputs {
    pub const fn new(parameter_18e: u8, parameter_ee: u8) -> Self {
        Self {
            parameter_18e,
            parameter_ee,
        }
    }

    const fn command(self, index: u8) -> Option<(u8, u8, u8)> {
        let parameter_ee_plus_two = self.parameter_ee.wrapping_add(2);
        match index {
            0 => Some((0x6b, 0x01, 0x01)),
            1 => Some((0x6b, 0x02, 0x73)),
            2 => Some((0x6b, 0x03, 0xba)),
            3 => Some((0x6b, 0x04, 0x88)),
            4 => Some((0x6b, 0x0e, 0xf4)),
            5 => Some((0x6b, 0x09, 0x02)),
            6 => Some((0x6b, 0x07, 0xfd)),
            7 => Some((0x6b, 0x08, 0xbf)),
            8 => Some((0x6b, 0x05, 0x01)),
            9 => Some((0x6b, 0x06, 0x11)),
            10 => Some((0x6b, 0x0c, 0xa7)),
            11 => Some((0x6b, 0x0d, 0x77)),
            12 => Some((0x6b, 0x0a, 0x08)),
            13 => Some((0x6b, 0x0b, 0x04)),
            14 => Some((0x6b, 0x0f, 0x81)),
            15 => Some((0x62, 0x00, 0x68)),
            16 => Some((0x62, 0x04, 0xa8)),
            17 => Some((0x62, 0x0f, self.parameter_18e)),
            18 => Some((0x62, 0x0b, 0x44)),
            19 => Some((0x62, 0x15, 0x08)),
            20 => Some((0x63, 0x06, 0x00)),
            21 => Some((0x62, 0x0d, 0x0a)),
            22 => Some((0x67, 0x02, 0x27)),
            23 => Some((0x66, 0x02, 0x70)),
            24 => Some((0x67, 0x18, parameter_ee_plus_two)),
            25 => Some((0x67, 0x19, parameter_ee_plus_two)),
            _ => None,
        }
    }
}

/// One complete PAC-owned PHY-I²C configuration operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationOperation {
    ConfigureAdcRate(PhyAdcRate),
    BiasRegisters,
    /// Clear the reviewed BBPLL calibration field and consume the vendor's
    /// final readback without exposing its register image.
    EnableBbpllCalibration,
    FilterDcap(PhyFilterDcapInputs),
    InitializationStageOne(PhyI2cInitializationStageOneInputs),
    RcCalibrationSettings,
    Sar2Initialization,
}

impl PhyI2cConfigurationOperation {
    /// Return the command at `index`, or `None` after the last command.
    pub const fn command(self, index: u8) -> Option<PhyI2cConfigurationCommand> {
        let write = match self {
            Self::ConfigureAdcRate(rate) => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::ADC_RATE_CONFIGURATION,
                        rate.analog_configuration_field_value(),
                    )),
                    _ => None,
                };
            }
            Self::BiasRegisters => bias_register_command(index),
            Self::EnableBbpllCalibration => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::ADC_RATE_CONFIGURATION,
                        0,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Read(PhyI2cAddress::recovered(
                        0x66, 0x04,
                    ))),
                    _ => None,
                };
            }
            Self::FilterDcap(inputs) => inputs.command(index),
            Self::InitializationStageOne(inputs) => inputs.command(index),
            Self::RcCalibrationSettings => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::RC_CONFIGURATION_0,
                        3,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::RC_CONFIGURATION_1,
                        1,
                    )),
                    2 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::RC_CONFIGURATION_2,
                        9,
                    )),
                    _ => None,
                };
            }
            Self::Sar2Initialization => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        analog_registers::TEMPERATURE_SENSOR_SAR2_STATUS,
                        0x05,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Write(
                        PhyI2cAddress::recovered(0x69, 0x03),
                        0x78,
                    )),
                    _ => None,
                };
            }
        };
        match write {
            Some((block, register, value)) => Some(PhyI2cConfigurationCommand::Write(
                PhyI2cAddress::recovered(block, register),
                value,
            )),
            None => None,
        }
    }

    /// Number of commands in this operation.
    pub const fn command_count(self) -> u8 {
        match self {
            Self::ConfigureAdcRate(_) => 1,
            Self::BiasRegisters => PHY_BIAS_REGISTER_COMMAND_COUNT,
            Self::EnableBbpllCalibration => PHY_BBPLL_CALIBRATION_COMMAND_COUNT,
            Self::FilterDcap(_) => PHY_FILTER_DCAP_COMMAND_COUNT,
            Self::InitializationStageOne(_) => PHY_I2C_INITIALIZATION_STAGE_ONE_COMMAND_COUNT,
            Self::RcCalibrationSettings => PHY_RC_CALIBRATION_SETTINGS_COMMAND_COUNT,
            Self::Sar2Initialization => PHY_SAR2_INITIALIZATION_COMMAND_COUNT,
        }
    }
}

/// One of the four Bluetooth TX-power analog-control byte registers.
///
/// The same block-0x67 registers carry the filter-DCAP parameter `0xf0`.
/// SOURCE(esp32s31): `librftest.a[phy_test.o]::phy_test_filter_band_set`
/// writes parameter byte `0xef` or `0xf0` to both `Low0` and `Low1`
/// depending on its selector, so these two registers also select a filter
/// band; which band each value selects is not established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlRegister {
    Low0,
    Low1,
    High0,
    High1,
}

impl BluetoothTxPowerControlRegister {
    const fn address(self) -> u8 {
        match self {
            Self::Low0 => 0x1c,
            Self::Low1 => 0x1d,
            Self::High0 => 0x1e,
            Self::High1 => 0x1f,
        }
    }
}

impl RadioPhyRegisters {
    /// Configure the reviewed host map and select the typed host for one
    /// opaque analog-register identity.
    pub fn configure_and_select_phy_i2c_host(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        self.configure_phy_i2c_host_map();
        block.host()
    }

    /// Start one opaque analog-register read without exposing command fields.
    pub fn try_start_phy_i2c_read(
        &mut self,
        address: PhyI2cAddress,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(address.block);
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_read_mask(address.block);
        self.publish_phy_i2c_command(host, address.block.code, address.register, 0, false);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque read.
    pub fn try_finish_phy_i2c_read(&self, address: PhyI2cAddress) -> Result<u8, PhyI2cAccessError> {
        let host = address.host();
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(self.sample_phy_i2c_result(host))
        }
    }

    /// Start one opaque analog-register write without exposing command fields.
    pub fn try_start_phy_i2c_write(
        &mut self,
        address: PhyI2cAddress,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(address.block);
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_command(host, address.block.code, address.register, value, true);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque write.
    pub fn try_finish_phy_i2c_write(
        &self,
        address: PhyI2cAddress,
    ) -> Result<(), PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(address.host()) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }

    /// Install the complete reviewed PHY-I²C host map with one fresh RMW.
    fn configure_phy_i2c_host_map(&mut self) {
        crate::generated::configure_phy_i2c_host_map(&self.peripherals.i2c_ana_mst);
    }

    /// Select the parallel host map used by the retained-wake stage two.
    pub fn select_phy_i2c_parallel_host_map(&mut self) {
        crate::generated::configure_phy_i2c_parallel_host_map(&self.peripherals.i2c_ana_mst);
    }

    /// Restore the normal radio host map with one fresh RMW.
    pub fn restore_phy_i2c_radio_host_map(&mut self) {
        self.configure_phy_i2c_host_map();
    }

    /// Start both writes of one parallel pair, host zero first.
    pub fn start_phy_i2c_parallel_pair(&mut self, pair: PhyI2cParallelWrite) {
        let (block, register, value) = pair.first;
        self.publish_phy_i2c_command(PhyI2cHost::Host0, block, register, value, true);
        let (block, register, value) = pair.second;
        self.publish_phy_i2c_command(PhyI2cHost::Host1, block, register, value, true);
    }

    /// Start one Bluetooth TX-power control read with its block read mask.
    pub fn start_bluetooth_tx_power_control_read(
        &mut self,
        register: BluetoothTxPowerControlRegister,
    ) -> Result<(), PhyI2cAccessError> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_read_mask(PhyI2cBlock::recovered(0x67));
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), 0, false);
        Ok(())
    }

    /// Start one Bluetooth TX-power control write.
    pub fn start_bluetooth_tx_power_control_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), value, true);
        Ok(())
    }

    /// Consume one completion edge of a read on `host`.
    pub fn finish_phy_i2c_host_read(&self, host: PhyI2cHost) -> Result<u8, PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(self.sample_phy_i2c_result(host))
        }
    }

    /// Consume one completion edge of a write on `host`.
    pub fn finish_phy_i2c_host_write(&self, host: PhyI2cHost) -> Result<(), PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }

    /// Publish the finite reset command for one analog-I²C host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        match host {
            PhyI2cHost::Host0 => crate::svd::fixed_register_image::pulse_phy_i2c_host0_reset(
                &self.peripherals.i2c_ana_mst,
            ),
            PhyI2cHost::Host1 => crate::svd::fixed_register_image::pulse_phy_i2c_host1_reset(
                &self.peripherals.i2c_ana_mst,
            ),
        }
    }

    /// Sample the reviewed completion predicate for one host.
    pub fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => {
                crate::svd::field_read::observe_phy_i2c_host0_busy(&self.peripherals.i2c_ana_mst)
            }
            PhyI2cHost::Host1 => {
                crate::svd::field_read::observe_phy_i2c_host1_busy(&self.peripherals.i2c_ana_mst)
            }
        }
    }

    /// Publish the complete complemented read mask used by the vendor leaf.
    fn publish_phy_i2c_read_mask(&mut self, block: PhyI2cBlock) {
        crate::svd::zero_based_field_write::publish_phy_i2c_read_mask(
            &self.peripherals.i2c_ana_mst,
            block.read_mask_complement_low(),
            0xff,
        );
    }

    /// Publish one complete host command in the reviewed vendor order.
    fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            PhyI2cHost::Host0 => crate::svd::zero_based_field_write::publish_phy_i2c_host0_command(
                &self.peripherals.i2c_ana_mst,
                block,
                register,
                value,
                write,
                true,
            ),
            PhyI2cHost::Host1 => crate::svd::zero_based_field_write::publish_phy_i2c_host1_command(
                &self.peripherals.i2c_ana_mst,
                block,
                register,
                value,
                write,
                true,
            ),
        }
    }

    /// Sample the completed data byte from one host.
    fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8 {
        match host {
            PhyI2cHost::Host0 => {
                crate::svd::field_read::observe_phy_i2c_host0_data(&self.peripherals.i2c_ana_mst)
            }
            PhyI2cHost::Host1 => {
                crate::svd::field_read::observe_phy_i2c_host1_data(&self.peripherals.i2c_ana_mst)
            }
        }
    }

    /// Apply all six timing RMWs in the complete vendor order.
    pub fn configure_phy_i2c_clock_selection(&mut self) {
        let registers = &self.peripherals.i2c_ana_mst;

        crate::generated::configure_phy_i2c_host0_sda_guard(registers);
        crate::generated::configure_phy_i2c_host0_scl_duration(registers);
        crate::generated::configure_phy_i2c_host1_sda_guard(registers);
        crate::generated::configure_phy_i2c_host1_scl_duration(registers);
        crate::generated::configure_phy_i2c_hardware_sda_guard(registers);
        crate::generated::configure_phy_i2c_hardware_scl_duration(registers);
    }

    /// Select register mode two, then enable it with a separate fresh RMW.
    pub fn configure_phy_i2c_master_registers(&mut self) {
        let registers = &self.peripherals.i2c_ana_mst;
        crate::generated::select_phy_i2c_register_mode(registers);
        crate::generated::enable_phy_i2c_register_access(registers);
    }

    /// Select one of the two complete-ROM BBPLL calibration encodings.
    pub fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        let state = if enabled {
            PhyI2cBbpllCalibrationState::Enabled
        } else {
            PhyI2cBbpllCalibrationState::Disabled
        };
        crate::generated::set_phy_i2c_bbpll_calibration(&self.peripherals.i2c_ana_mst, state);
    }

    /// Program the complete reviewed 45-entry PHY-I²C command memory.
    ///
    /// The PAC owns every destination block, byte-register, command index and
    /// fixed or derived register image. Callers provide only the six retained
    /// vendor-parameter facts.
    pub fn configure_phy_i2c_command_memory(&mut self, inputs: PhyI2cCommandMemoryInputs) {
        let dynamic_values = inputs.dynamic_values();
        let mut index = 0;
        let mut dynamic_cursor = 0;
        while index != PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT {
            let (block, register, fixed_value) = PHY_I2C_COMMAND_MEMORY_TEMPLATE[index];
            let value = if dynamic_cursor != PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES.len()
                && PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES[dynamic_cursor] == index
            {
                let value = dynamic_values[dynamic_cursor];
                dynamic_cursor += 1;
                value
            } else {
                fixed_value
            };
            crate::svd::zero_based_field_write::phy_i2c_command_memory(
                &self.peripherals.phy_i2c_command_ram,
                index,
                block,
                register,
                value,
            );
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PhyI2cAddress, analog_registers};

    #[test]
    fn the_analog_image_reads_each_reviewed_register_once() {
        let image: [PhyI2cAddress; PhyI2cAddress::IMAGE_LEN] =
            core::array::from_fn(|index| PhyI2cAddress::image(index).unwrap());
        assert_eq!(PhyI2cAddress::image(PhyI2cAddress::IMAGE_LEN), None);
        for (index, address) in image.iter().enumerate() {
            assert!(!image[..index].contains(address));
        }
        for address in [
            analog_registers::XTAL_DUTY_CANDIDATE,
            analog_registers::TX_CAPACITOR_BANKS,
            analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS.address(),
            analog_registers::RFPLL_CAPACITOR_CORRECTION_DIRECTION.address(),
        ] {
            assert_eq!(image.iter().filter(|&&a| a == address).count(), 1);
        }
    }
}
