//! Ownership-bound access to the shared PHY analog-I²C master.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::PhyI2cBbpllCalibrationState;

use crate::analog_i2c::{self, Address, Field, Host, Profile, Route, fields, registers};

const fn normal_route(address: Address) -> Route {
    match analog_i2c::route(Profile::LibphyNormal, address.block()) {
        Some(route) => route,
        None => panic!("production analog address requires a normal route"),
    }
}

/// One finite analog-register command is still owned by its hardware host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cAccessError {
    Busy,
}

const PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT: usize = 45;

// Complete command order recovered from
// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. Values which depend on
// the explicit PHY parameter snapshot are replaced by
// `PhyI2cCommandMemoryInputs::dynamic_values` below.
const PHY_I2C_COMMAND_MEMORY_TEMPLATE: [(Address, u8); PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT] = [
    (registers::BBTOP_RF_CLOSE_CONTROL, 0x07),
    (registers::TXRF_BYTE_01_OPAQUE, 0x01),
    (registers::TXRF_TX_CAPACITOR_LOW, 0x73),
    (registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_0, 0xba),
    (registers::TXRF_BYTE_04_OPAQUE, 0x88),
    (registers::TXRF_BYTE_05_OPAQUE, 0x01),
    (registers::TXRF_BYTE_06_OPAQUE, 0x11),
    (registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_1, 0xfd),
    (registers::TXRF_BYTE_08_OPAQUE, 0xbf),
    (registers::TXRF_BYTE_09_OPAQUE, 0x02),
    (registers::TXRF_BYTE_0A_OPAQUE, 0x08),
    (registers::TXRF_BYTE_0B_OPAQUE, 0x04),
    (registers::TXRF_BYTE_0C_OPAQUE, 0xa7),
    (registers::TXRF_BYTE_0D_OPAQUE, 0x77),
    (registers::TXRF_BYTE_0E_OPAQUE, 0xf4),
    (registers::TXRF_RC_CONFIGURATION_1, 0x81),
    (registers::RFPLL_RESTART_HIGH, 0x68),
    (registers::RFPLL_DCODE_CKGEN_RESET, 0xa8),
    (registers::RFPLL_CAPACITOR_SEARCH_ENABLE, 0x44),
    (registers::RFPLL_BYTE_0D_OPAQUE, 0x0a),
    (registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE, 0x00),
    (registers::RFPLL_BYTE_15_OPAQUE, 0x08),
    (registers::BBPLL_I2C_BBPLL_OC_REF_DIV, 0x70),
    (registers::BBTOP_RF_CLOSE_CONTROL, 0x27),
    (registers::BBTOP_BYTE_04_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_05_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_06_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_07_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_0C_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_0D_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_0E_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_0F_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_14_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_15_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_16_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_17_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_18_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_19_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_1C_OPAQUE, 0x00),
    (registers::BBTOP_BYTE_1D_OPAQUE, 0x00),
    (registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_0, 0x00),
    (registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_1, 0x00),
    (registers::SDM_RFPLL_SDM_LOW, 0x00),
    (registers::BIAS_I2C_BIAS_DREG_1P6, 0xaf),
    (registers::BIAS_I2C_BIAS_DREG_1P1_PVT, 0x7f),
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
            fields::BBTOP_BLUETOOTH_TX_POWER_HIGH_0_BIT_6_OPAQUE
                .insert_truncated(self.parameter_f0, 1),
            self.parameter_f0,
        ]
    }

    /// Return the retained-wake stage-two pair at `index`, or `None` after the last.
    pub const fn initialization_stage_two_pair(self, index: usize) -> Option<PhyI2cParallelWrite> {
        if index >= PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT {
            return None;
        }

        const FIRST_ADDRESSES: [Address; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            registers::TXRF_TX_CAPACITOR_LOW,
            registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_0,
            registers::TXRF_BYTE_04_OPAQUE,
            registers::TXRF_BYTE_0E_OPAQUE,
            registers::TXRF_BYTE_09_OPAQUE,
            registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_1,
            registers::TXRF_BYTE_08_OPAQUE,
            registers::TXRF_BYTE_05_OPAQUE,
            registers::TXRF_BYTE_06_OPAQUE,
            registers::TXRF_BYTE_0C_OPAQUE,
            registers::TXRF_BYTE_0D_OPAQUE,
            registers::TXRF_BYTE_0A_OPAQUE,
            registers::TXRF_BYTE_0B_OPAQUE,
            registers::TXRF_RC_CONFIGURATION_1,
            registers::TXRF_BYTE_01_OPAQUE,
            registers::RFPLL_RESTART_HIGH,
            registers::RFPLL_DCODE_CKGEN_RESET,
            registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE,
            registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
            registers::RFPLL_BYTE_0D_OPAQUE,
            registers::RFPLL_BYTE_15_OPAQUE,
            registers::BBPLL_I2C_BBPLL_OC_REF_DIV,
        ];
        const FIRST_FIXED_VALUES: [u8; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            0x73, 0xba, 0x88, 0xf4, 0x02, 0xfd, 0xbf, 0x01, 0x11, 0xa7, 0x77, 0x08, 0x04, 0x81,
            0x01, 0x68, 0xa8, 0, 0x44, 0x0a, 0x08, 0x70,
        ];
        const SECOND_ADDRESSES: [Address; PHY_I2C_INITIALIZATION_STAGE_TWO_PAIR_COUNT] = [
            registers::BBTOP_RF_CLOSE_CONTROL,
            registers::BBTOP_BYTE_14_OPAQUE,
            registers::BBTOP_BYTE_15_OPAQUE,
            registers::BBTOP_BYTE_16_OPAQUE,
            registers::BBTOP_BYTE_17_OPAQUE,
            registers::BBTOP_BYTE_18_OPAQUE,
            registers::BBTOP_BYTE_19_OPAQUE,
            registers::BBTOP_BYTE_1C_OPAQUE,
            registers::BBTOP_BYTE_1D_OPAQUE,
            registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_0,
            registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_1,
            registers::BBTOP_BYTE_04_OPAQUE,
            registers::BBTOP_BYTE_05_OPAQUE,
            registers::BBTOP_BYTE_06_OPAQUE,
            registers::BBTOP_BYTE_07_OPAQUE,
            registers::BBTOP_BYTE_0C_OPAQUE,
            registers::BBTOP_BYTE_0D_OPAQUE,
            registers::BBTOP_BYTE_0E_OPAQUE,
            registers::BBTOP_BYTE_0F_OPAQUE,
            registers::SDM_RFPLL_SDM_LOW,
            registers::SDM_RFPLL_SDM_LOW,
            registers::SDM_RFPLL_SDM_LOW,
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
            9 => fields::BBTOP_BLUETOOTH_TX_POWER_HIGH_0_BIT_6_OPAQUE
                .insert_truncated(self.parameter_f0, 1),
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
            first: (FIRST_ADDRESSES[index], first_value),
            second: (SECOND_ADDRESSES[index], second_value),
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
    first: (Address, u8),
    second: (Address, u8),
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
    #[cfg(oer_probe)]
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
    Read(Address),
    Write(Address, u8),
    Modify(Field, u8),
}

impl PhyI2cConfigurationCommand {
    /// The byte register addressed by this command.
    pub const fn address(self) -> Address {
        match self {
            Self::Read(address) | Self::Write(address, _) => address,
            Self::Modify(field, _) => field.address(),
        }
    }
}

const fn bias_register_command(index: u8) -> Option<(Address, u8)> {
    match index {
        0 => Some((registers::BIAS_I2C_BIAS_DREG_1P6, 0xaf)),
        1 => Some((registers::BIAS_I2C_BIAS_DREG_1P1_PVT, 0x7f)),
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

    const fn command(self, index: u8) -> Option<(Address, u8)> {
        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        match index {
            0 => Some((registers::BBTOP_BYTE_14_OPAQUE, high_filter)),
            1 => Some((registers::BBTOP_BYTE_15_OPAQUE, high_filter)),
            2 => Some((registers::BBTOP_BYTE_16_OPAQUE, low_filter)),
            3 => Some((registers::BBTOP_BYTE_17_OPAQUE, self.parameter_ed)),
            4 => Some((registers::BBTOP_BYTE_18_OPAQUE, self.parameter_ee)),
            5 => Some((registers::BBTOP_BYTE_19_OPAQUE, self.parameter_ee)),
            6 => Some((registers::BBTOP_BYTE_1C_OPAQUE, self.parameter_f0)),
            7 => Some((registers::BBTOP_BYTE_1D_OPAQUE, self.parameter_f0)),
            8 => Some((
                registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_0,
                fields::BBTOP_BLUETOOTH_TX_POWER_HIGH_0_BIT_6_OPAQUE
                    .insert_truncated(self.parameter_f0, 1),
            )),
            9 => Some((
                registers::BBTOP_BLUETOOTH_TX_POWER_HIGH_1,
                self.parameter_f0,
            )),
            10 => Some((registers::BBTOP_BYTE_04_OPAQUE, self.parameter_e9)),
            11 => Some((registers::BBTOP_BYTE_05_OPAQUE, self.parameter_e9)),
            12 => Some((registers::BBTOP_BYTE_06_OPAQUE, self.parameter_ea)),
            13 => Some((registers::BBTOP_BYTE_07_OPAQUE, self.parameter_ea)),
            14 => Some((registers::BBTOP_BYTE_0C_OPAQUE, self.parameter_e9)),
            15 => Some((registers::BBTOP_BYTE_0D_OPAQUE, self.parameter_e9)),
            16 => Some((registers::BBTOP_BYTE_0E_OPAQUE, self.parameter_ea)),
            17 => Some((registers::BBTOP_BYTE_0F_OPAQUE, self.parameter_ea)),
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

    const fn command(self, index: u8) -> Option<(Address, u8)> {
        let parameter_ee_plus_two = self.parameter_ee.wrapping_add(2);
        match index {
            0 => Some((registers::TXRF_BYTE_01_OPAQUE, 0x01)),
            1 => Some((registers::TXRF_TX_CAPACITOR_LOW, 0x73)),
            2 => Some((registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_0, 0xba)),
            3 => Some((registers::TXRF_BYTE_04_OPAQUE, 0x88)),
            4 => Some((registers::TXRF_BYTE_0E_OPAQUE, 0xf4)),
            5 => Some((registers::TXRF_BYTE_09_OPAQUE, 0x02)),
            6 => Some((registers::TXRF_WIFI_TX_TEMPERATURE_TRACKING_1, 0xfd)),
            7 => Some((registers::TXRF_BYTE_08_OPAQUE, 0xbf)),
            8 => Some((registers::TXRF_BYTE_05_OPAQUE, 0x01)),
            9 => Some((registers::TXRF_BYTE_06_OPAQUE, 0x11)),
            10 => Some((registers::TXRF_BYTE_0C_OPAQUE, 0xa7)),
            11 => Some((registers::TXRF_BYTE_0D_OPAQUE, 0x77)),
            12 => Some((registers::TXRF_BYTE_0A_OPAQUE, 0x08)),
            13 => Some((registers::TXRF_BYTE_0B_OPAQUE, 0x04)),
            14 => Some((registers::TXRF_RC_CONFIGURATION_1, 0x81)),
            15 => Some((registers::RFPLL_RESTART_HIGH, 0x68)),
            16 => Some((registers::RFPLL_DCODE_CKGEN_RESET, 0xa8)),
            17 => Some((
                registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE,
                self.parameter_18e,
            )),
            18 => Some((registers::RFPLL_CAPACITOR_SEARCH_ENABLE, 0x44)),
            19 => Some((registers::RFPLL_BYTE_15_OPAQUE, 0x08)),
            20 => Some((registers::SDM_RFPLL_SDM_LOW, 0x00)),
            21 => Some((registers::RFPLL_BYTE_0D_OPAQUE, 0x0a)),
            22 => Some((registers::BBTOP_RF_CLOSE_CONTROL, 0x27)),
            23 => Some((registers::BBPLL_I2C_BBPLL_OC_REF_DIV, 0x70)),
            24 => Some((registers::BBTOP_BYTE_18_OPAQUE, parameter_ee_plus_two)),
            25 => Some((registers::BBTOP_BYTE_19_OPAQUE, parameter_ee_plus_two)),
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
                        fields::BBPLL_ADC_RATE_CONFIGURATION,
                        rate.analog_configuration_field_value(),
                    )),
                    _ => None,
                };
            }
            Self::BiasRegisters => bias_register_command(index),
            Self::EnableBbpllCalibration => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        fields::BBPLL_ADC_RATE_CONFIGURATION,
                        0,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Read(
                        registers::BBPLL_I2C_BBPLL_RSTB_DIV_ADC,
                    )),
                    _ => None,
                };
            }
            Self::FilterDcap(inputs) => inputs.command(index),
            Self::InitializationStageOne(inputs) => inputs.command(index),
            Self::RcCalibrationSettings => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        fields::TXRF_RC_CONFIGURATION_0,
                        3,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Modify(
                        fields::TXRF_RC_CONFIGURATION_1,
                        1,
                    )),
                    2 => Some(PhyI2cConfigurationCommand::Modify(
                        fields::TXRF_RC_CONFIGURATION_2,
                        9,
                    )),
                    _ => None,
                };
            }
            Self::Sar2Initialization => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify(
                        fields::SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR,
                        0x05,
                    )),
                    1 => Some(PhyI2cConfigurationCommand::Write(
                        registers::SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR,
                        0x78,
                    )),
                    _ => None,
                };
            }
        };
        match write {
            Some((address, value)) => Some(PhyI2cConfigurationCommand::Write(address, value)),
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
    pub fn configure_and_select_phy_i2c_host(&mut self, block: Route) -> Host {
        self.configure_phy_i2c_host_map();
        block.host
    }

    /// Start one opaque analog-register read without exposing command fields.
    pub fn try_start_phy_i2c_read(&mut self, address: Address) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(normal_route(address));
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_read_mask(normal_route(address));
        self.publish_phy_i2c_command(host, address.block(), address.index(), 0, false);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque read.
    pub fn try_finish_phy_i2c_read(&self, address: Address) -> Result<u8, PhyI2cAccessError> {
        let host = normal_route(address).host;
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(self.sample_phy_i2c_result(host))
        }
    }

    /// Start one opaque analog-register write without exposing command fields.
    pub fn try_start_phy_i2c_write(
        &mut self,
        address: Address,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(normal_route(address));
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_command(host, address.block(), address.index(), value, true);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque write.
    pub fn try_finish_phy_i2c_write(&self, address: Address) -> Result<(), PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(normal_route(address).host) {
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
        let (address, value) = pair.first;
        self.publish_phy_i2c_command(Host::Host0, address.block(), address.index(), value, true);
        let (address, value) = pair.second;
        self.publish_phy_i2c_command(Host::Host1, address.block(), address.index(), value, true);
    }

    /// Start one Bluetooth TX-power control read with its block read mask.
    pub fn start_bluetooth_tx_power_control_read(
        &mut self,
        register: BluetoothTxPowerControlRegister,
    ) -> Result<(), PhyI2cAccessError> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(Host::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_read_mask(normal_route(registers::BBTOP_RF_CLOSE_CONTROL));
        self.publish_phy_i2c_command(Host::Host1, 0x67, register.address(), 0, false);
        Ok(())
    }

    /// Start one Bluetooth TX-power control write.
    pub fn start_bluetooth_tx_power_control_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(Host::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_command(Host::Host1, 0x67, register.address(), value, true);
        Ok(())
    }

    /// Consume one completion edge of a read on `host`.
    pub fn finish_phy_i2c_host_read(&self, host: Host) -> Result<u8, PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(self.sample_phy_i2c_result(host))
        }
    }

    /// Consume one completion edge of a write on `host`.
    pub fn finish_phy_i2c_host_write(&self, host: Host) -> Result<(), PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }

    /// Publish the finite reset command for one analog-I²C host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: Host) {
        match host {
            Host::Host0 => crate::svd::fixed_register_image::pulse_phy_i2c_host0_reset(
                &self.peripherals.i2c_ana_mst,
            ),
            Host::Host1 => crate::svd::fixed_register_image::pulse_phy_i2c_host1_reset(
                &self.peripherals.i2c_ana_mst,
            ),
        }
    }

    /// Sample the reviewed completion predicate for one host.
    pub fn phy_i2c_master_is_busy(&self, host: Host) -> bool {
        match host {
            Host::Host0 => {
                crate::svd::field_read::observe_phy_i2c_host0_busy(&self.peripherals.i2c_ana_mst)
            }
            Host::Host1 => {
                crate::svd::field_read::observe_phy_i2c_host1_busy(&self.peripherals.i2c_ana_mst)
            }
        }
    }

    /// Publish the complete complemented read mask used by the vendor leaf.
    fn publish_phy_i2c_read_mask(&mut self, block: Route) {
        crate::svd::zero_based_field_write::publish_phy_i2c_read_mask(
            &self.peripherals.i2c_ana_mst,
            block.complemented_read_select(),
            0xff,
        );
    }

    /// Publish one complete host command in the reviewed vendor order.
    fn publish_phy_i2c_command(
        &mut self,
        host: Host,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            Host::Host0 => crate::svd::zero_based_field_write::publish_phy_i2c_host0_command(
                &self.peripherals.i2c_ana_mst,
                block,
                register,
                value,
                write,
                true,
            ),
            Host::Host1 => crate::svd::zero_based_field_write::publish_phy_i2c_host1_command(
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
    fn sample_phy_i2c_result(&self, host: Host) -> u8 {
        match host {
            Host::Host0 => {
                crate::svd::field_read::observe_phy_i2c_host0_data(&self.peripherals.i2c_ana_mst)
            }
            Host::Host1 => {
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
            let (address, fixed_value) = PHY_I2C_COMMAND_MEMORY_TEMPLATE[index];
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
                address.block(),
                address.index(),
                value,
            );
            index += 1;
        }
    }
}
