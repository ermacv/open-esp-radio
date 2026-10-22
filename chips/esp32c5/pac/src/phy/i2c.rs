//! Ownership-bound access to the analog register I2C master the PHY drives.
//!
//! The libphy leaves `phy_chip_i2c_readReg` and `phy_chip_i2c_writeReg`
//! busy-wait on bit 25 of the host command word. This owner does not
//! reproduce those loops: it separates command publication from completion
//! observation so an outer owner can wait and inspect the host once.
//!
//! SOURCE: reviewed evidence `C5_BLOB_LIBPHY_PHY_I2C` (ESP32-C5
//! `libphy.a[phy_i2c.o]`: `libphy[phy_i2c.o]::phy_chip_i2c_readReg`, `libphy[phy_i2c.o]::phy_chip_i2c_readReg_org`,
//! `libphy[phy_i2c.o]::phy_chip_i2c_writeReg`, `libphy[phy_i2c.o]::phy_get_i2c_hostid_`, `libphy[phy_i2c.o]::phy_get_i2c_read_mask_`),
//! corroborated by `ESP_IDF_4D59230D_C5_I2C_ANA_MST`.

#![forbid(unsafe_code)]

#[cfg(any(test, feature = "test-support", oer_probe))]
use crate::analog_i2c::{Address, Field, Host, Request, Route, fields, registers};
use crate::svd;

/// One command of each host in a parallel write, as `phy_i2c_paral_write`
/// publishes them: host 0 first, then host 1, without the start bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cParallelWrite {
    first: (Address, u8),
    second: (Address, u8),
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cParallelWrite {
    /// The (block, register, data) commands of host 0 and host 1.
    pub const fn new(first: (Address, u8), second: (Address, u8)) -> Self {
        Self { first, second }
    }

    pub const fn first(self) -> (Address, u8) {
        self.first
    }

    pub const fn second(self) -> (Address, u8) {
        self.second
    }
}

/// `phy_get_data_sat(value, high, low)`.
#[cfg(any(test, feature = "test-support", oer_probe))]
const fn clamp(value: u8, low: u8, high: u8) -> u8 {
    if high < value {
        high
    } else if value < low {
        low
    } else {
        value
    }
}

/// The `phy_param` fields `phy_i2c_init1` reads, named by their offset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cInitializationInputs {
    pub parameter_f5: u8,
    pub parameter_f6: u8,
    pub parameter_f7: u8,
    pub parameter_f8: u8,
    pub parameter_f9: u8,
    pub parameter_fa: u8,
    pub parameter_fb: u8,
    pub parameter_fc: u8,
    /// Low byte of the halfword at 0x410.
    pub parameter_410: u8,
    /// Low byte of the halfword at 0x412.
    pub parameter_412: u8,
    /// The halfword at 0x416.
    pub parameter_416: u16,
}

/// Pairs of the parallel sequence of `phy_i2c_init1`.
#[cfg(any(test, feature = "test-support", oer_probe))]
pub const PHY_I2C_INITIALIZATION_PAIR_COUNT: usize = 44;

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cInitializationInputs {
    /// Pair `index` of the 44-pair parallel sequence `phy_i2c_init1` issues.
    ///
    /// SOURCE: reviewed evidence `C5_BLOB_LIBPHY_PHY_I2C_INIT1`.
    pub const fn pair(self, index: usize) -> Option<PhyI2cParallelWrite> {
        let (first, second) = match index {
            0 => (
                (registers::TXRF_BYTE_02_OPAQUE, 0x72),
                (registers::BIAS_BYTE_00_OPAQUE, 0xff),
            ),
            1 => (
                (registers::TXRF_BYTE_03_OPAQUE, 0xaa),
                (registers::BIAS_BYTE_01_OPAQUE, 0x7f),
            ),
            2 => (
                (registers::TXRF_BYTE_0E_OPAQUE, 0x55),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            3 => (
                (registers::TXRF_BYTE_07_OPAQUE, 0xff),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            4 => (
                (registers::TXRF_BYTE_0A_OPAQUE, 0x8),
                (registers::BBTOP_BYTE_02_OPAQUE, 0x3),
            ),
            5 => (
                (registers::TXRF_BYTE_0C_OPAQUE, 0x55),
                (registers::BBTOP_BYTE_01_OPAQUE, 0x6f),
            ),
            6 => (
                (registers::TXRF_RC_CALIBRATION_SECOND, 0x81),
                (registers::BBTOP_BYTE_05_OPAQUE, 0x6b),
            ),
            7 => (
                (registers::TXRF_BYTE_09_OPAQUE, 0x0),
                (registers::BBTOP_BYTE_1D_OPAQUE, 0xc2),
            ),
            8 => (
                (registers::TXRF_BYTE_05_OPAQUE, 0x33),
                (registers::BBTOP_BYTE_06_OPAQUE, self.parameter_f5),
            ),
            9 => (
                (registers::TXRF_BYTE_06_OPAQUE, 0x30),
                (
                    registers::BBTOP_BYTE_08_OPAQUE,
                    clamp(self.parameter_f7, 10, 50),
                ),
            ),
            10 => (
                (registers::TXRF_BYTE_0D_OPAQUE, 0x57),
                (registers::BBTOP_BYTE_0A_OPAQUE, self.parameter_f5),
            ),
            11 => (
                (registers::TXRF_BYTE_08_OPAQUE, 0xfd),
                (registers::BBTOP_BYTE_0C_OPAQUE, self.parameter_f7),
            ),
            12 => (
                (registers::TXRF_BYTE_04_OPAQUE, 0xac),
                (registers::BBTOP_BYTE_07_OPAQUE, self.parameter_f6),
            ),
            13 => (
                (
                    registers::TXTOP_BYTE_05_OPAQUE,
                    crate::generated::layouts::PhyI2cCommandWord::from_bits(
                        crate::generated::PhyI2cInitializationHighCommand::compose(
                            self.parameter_416,
                        )
                        .get(),
                    )
                    .data(),
                ),
                (
                    registers::BBTOP_BYTE_09_OPAQUE,
                    clamp(self.parameter_f8, 10, 60),
                ),
            ),
            14 => (
                (registers::TXTOP_BYTE_07_OPAQUE, 0x63),
                (registers::BBTOP_BYTE_0B_OPAQUE, self.parameter_f6),
            ),
            15 => (
                (registers::TXTOP_BYTE_08_OPAQUE, 0x73),
                (registers::BBTOP_BYTE_0D_OPAQUE, self.parameter_f8),
            ),
            16 => (
                (registers::TXTOP_BYTE_09_OPAQUE, 0xc),
                (registers::BBTOP_BYTE_0E_OPAQUE, self.parameter_fb),
            ),
            17 => (
                (registers::TXTOP_BYTE_0D_OPAQUE, 0x22),
                (registers::BBTOP_BYTE_10_OPAQUE, self.parameter_fb),
            ),
            18 => (
                (registers::TXTOP_BYTE_01_OPAQUE, 0x71),
                (registers::BBTOP_BYTE_12_OPAQUE, self.parameter_f9),
            ),
            19 => (
                (registers::TXTOP_BYTE_10_OPAQUE, 0x63),
                (registers::BBTOP_BYTE_14_OPAQUE, self.parameter_f9),
            ),
            20 => (
                (registers::TXTOP_BYTE_11_OPAQUE, 0x73),
                (registers::BBTOP_BYTE_0F_OPAQUE, self.parameter_fc),
            ),
            21 => (
                (registers::TXTOP_BYTE_04_OPAQUE, 0x47),
                (registers::BBTOP_BYTE_11_OPAQUE, self.parameter_fc),
            ),
            22 => (
                (
                    registers::TXTOP_BYTE_0C_OPAQUE,
                    crate::generated::layouts::PhyI2cCommandWord::from_bits(
                        crate::generated::PhyI2cInitializationLowCommand::compose(
                            self.parameter_416,
                        )
                        .get(),
                    )
                    .data(),
                ),
                (registers::BBTOP_BYTE_13_OPAQUE, self.parameter_fa),
            ),
            23 => (
                (registers::TXTOP_BYTE_0F_OPAQUE, 0x47),
                (registers::BBTOP_BYTE_15_OPAQUE, self.parameter_fa),
            ),
            24 => (
                (registers::TXTOP_BYTE_12_OPAQUE, 0x44),
                (registers::BIAS_BYTE_03_OPAQUE, 0xf),
            ),
            25 => (
                (registers::TXTOP_BYTE_13_OPAQUE, 0x63),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            26 => (
                (registers::TXTOP_BYTE_14_OPAQUE, 0x73),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            27 => (
                (registers::SDM_BYTE_0F_OPAQUE, 0x3f),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            28 => (
                (registers::SDM_BYTE_01_OPAQUE, 0xab),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            29 => (
                (registers::SDM_BYTE_13_OPAQUE, 0x90),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            30 => (
                (registers::SDM_BYTE_06_OPAQUE, 0xe0),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            31 => (
                (registers::SDM_BYTE_15_OPAQUE, 0x98),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            32 => (
                (registers::SDM_BYTE_11_OPAQUE, 0xc),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            33 => (
                (registers::SDM_BYTE_14_OPAQUE, 0xa),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            34 => (
                (registers::SDM_BYTE_00_OPAQUE, 0xdb),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            35 => (
                (registers::SDM_BYTE_0E_OPAQUE, 0x0),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            36 => (
                (registers::SDM_BYTE_08_OPAQUE, 0x61),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            37 => (
                (registers::SDM_BYTE_07_OPAQUE, 0x0),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            38 => (
                (registers::TXTOP_BYTE_0A_OPAQUE, self.parameter_410),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            39 => (
                (registers::TXTOP_BYTE_0B_OPAQUE, self.parameter_412),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            40 => (
                (registers::SDM_BYTE_1A_OPAQUE, 0x11),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            41 => (
                (registers::SDM_BYTE_19_OPAQUE, 0x48),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            42 => (
                (registers::SDM_BYTE_12_OPAQUE, 0x0),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            43 => (
                (registers::SDM_BYTE_18_OPAQUE, 0xdd),
                (registers::BBTOP_BYTE_03_OPAQUE, 0x44),
            ),
            _ => return None,
        };
        Some(PhyI2cParallelWrite::new(first, second))
    }
}

/// One command of an analog configuration leaf, in the vendor order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub enum PhyI2cConfigurationCommand {
    Write(Address, u8),
    Modify { field: Field, value: u8 },
}

/// The analog configuration leaves of `phy_rf_init` that only write
/// analog registers.
///
/// SOURCE: reviewed evidence `C5_BLOB_LIBPHY_RF_INIT_LEAVES` and
/// `C5_BLOB_LIBPHY_RF_INIT_LEAVES_3` (`libphy[phy_i2c.o]::phy_i2c_rc_cal_set`,
/// `libphy[phy_i2c.o]::phy_filter_dcap_set`, `libphy[phy_i2c.o]::phy_i2c_pkdet_set`, `libphy[phy_i2c.o]::phy_i2c_sar2_init_code`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub enum PhyI2cConfiguration {
    /// `phy_band_i2c_set`.
    Band,
    /// `phy_xtal_reg_set`.
    CrystalRegisters,
    /// The analog half of `phy_dac_rate_set`.
    DacRate,
    /// The analog half of `phy_adc_rate_set(rate)`.
    AdcRate(bool),
    /// `phy_bias_reg_set`.
    BiasRegisters,
    /// `phy_i2c_rc_cal_set(first, second, third)`.
    RcCalibration(PhyI2cRcCalibration),
    /// `phy_filter_dcap_set` over the `phy_param` fields it reads.
    FilterCapacitors(PhyI2cInitializationInputs),
    /// `phy_i2c_pkdet_set`.
    PeakDetector,
    /// `phy_i2c_sar2_init_code(code)`.
    Sar2InitializationCode(PhyI2cSar2Code),
}

/// Arguments of `phy_i2c_rc_cal_set`, within the fields they replace:
/// block 0x6B register 0x11 bits 5:4, register 0x0F bits 7:3 and register
/// 0x13 bits 5:2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cRcCalibration {
    first: u8,
    second: u8,
    third: u8,
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cRcCalibration {
    pub const fn new(first: u8, second: u8, third: u8) -> Option<Self> {
        if first <= 3 && second <= 31 && third <= 15 {
            Some(Self {
                first,
                second,
                third,
            })
        } else {
            None
        }
    }
}

/// Argument of `phy_i2c_sar2_init_code`: bits 11:8 go to block 0x69
/// register 4 bits 3:0 and bits 7:0 to register 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cSar2Code(u16);

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cSar2Code {
    pub const fn new(code: u16) -> Option<Self> {
        if code <= 0xfff {
            Some(Self(code))
        } else {
            None
        }
    }
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cConfiguration {
    /// Command `index`, or `None` after the last.
    pub const fn command(self, index: usize) -> Option<PhyI2cConfigurationCommand> {
        const fn modify(field: Field, value: u8) -> PhyI2cConfigurationCommand {
            PhyI2cConfigurationCommand::Modify { field, value }
        }
        Some(match (self, index) {
            (Self::Band, 0) => {
                PhyI2cConfigurationCommand::Write(registers::BIAS_BYTE_01_OPAQUE, 0x7f)
            }
            (Self::CrystalRegisters, 0) => modify(fields::ULP_BOD_BYTE_08_OPAQUE_BIT_4_OPAQUE, 0),
            (Self::CrystalRegisters, 1) => modify(fields::ULP_BOD_BYTE_07_OPAQUE_BIT_5_OPAQUE, 1),
            (Self::DacRate, 0) => modify(fields::BBPLL_I2C_BBPLL_DIV_DAC, 0),
            (Self::AdcRate(rate), 0) => modify(fields::BBPLL_ADC_RATE, if rate { 0 } else { 1 }),
            (Self::BiasRegisters, 0) => modify(fields::BIAS_BYTE_00_OPAQUE_BITS_0_3_OPAQUE, 0xf),
            (Self::BiasRegisters, 1) => modify(fields::BIAS_BYTE_01_OPAQUE_BITS_4_7_OPAQUE, 7),
            (Self::BiasRegisters, 2) => modify(fields::BIAS_BYTE_00_OPAQUE_BITS_4_7_OPAQUE, 9),
            (Self::BiasRegisters, 3) => modify(fields::BIAS_BYTE_01_OPAQUE_BITS_0_3_OPAQUE, 0xf),
            (Self::RcCalibration(arguments), 0) => {
                modify(fields::TXRF_RC_CALIBRATION_FIRST, arguments.first)
            }
            (Self::RcCalibration(arguments), 1) => {
                modify(fields::TXRF_RC_CALIBRATION_SECOND, arguments.second)
            }
            (Self::RcCalibration(arguments), 2) => {
                modify(fields::TXRF_RC_CALIBRATION_THIRD, arguments.third)
            }
            (Self::FilterCapacitors(_), 0) => {
                modify(fields::BBTOP_BYTE_1D_OPAQUE_BITS_2_3_OPAQUE, 0)
            }
            (Self::FilterCapacitors(_), 1) => modify(fields::BBTOP_BYTE_05_OPAQUE_BIT_6_OPAQUE, 1),
            (Self::FilterCapacitors(_), 2) => modify(fields::BBTOP_BYTE_05_OPAQUE_BIT_3_OPAQUE, 1),
            (Self::FilterCapacitors(_), 3) => modify(fields::BBTOP_BYTE_05_OPAQUE_BIT_5_OPAQUE, 1),
            (Self::FilterCapacitors(inputs), 4..=19) => {
                let (address, value) = match index {
                    4 => (registers::BBTOP_BYTE_06_OPAQUE, inputs.parameter_f5),
                    5 => (
                        registers::BBTOP_BYTE_08_OPAQUE,
                        clamp(inputs.parameter_f7, 10, 50),
                    ),
                    6 => (registers::BBTOP_BYTE_0A_OPAQUE, inputs.parameter_f5),
                    7 => (registers::BBTOP_BYTE_0C_OPAQUE, inputs.parameter_f7),
                    8 => (registers::BBTOP_BYTE_07_OPAQUE, inputs.parameter_f6),
                    9 => (
                        registers::BBTOP_BYTE_09_OPAQUE,
                        clamp(inputs.parameter_f8, 10, 60),
                    ),
                    10 => (registers::BBTOP_BYTE_0B_OPAQUE, inputs.parameter_f6),
                    11 => (registers::BBTOP_BYTE_0D_OPAQUE, inputs.parameter_f8),
                    12 => (registers::BBTOP_BYTE_0E_OPAQUE, inputs.parameter_fb),
                    13 => (registers::BBTOP_BYTE_10_OPAQUE, inputs.parameter_fb),
                    14 => (registers::BBTOP_BYTE_12_OPAQUE, inputs.parameter_f9),
                    15 => (registers::BBTOP_BYTE_14_OPAQUE, inputs.parameter_f9),
                    16 => (registers::BBTOP_BYTE_0F_OPAQUE, inputs.parameter_fc),
                    17 => (registers::BBTOP_BYTE_11_OPAQUE, inputs.parameter_fc),
                    18 => (registers::BBTOP_BYTE_13_OPAQUE, inputs.parameter_fa),
                    _ => (registers::BBTOP_BYTE_15_OPAQUE, inputs.parameter_fa),
                };
                PhyI2cConfigurationCommand::Write(address, value)
            }
            (Self::PeakDetector, 0) => modify(fields::BBTOP_BYTE_1D_OPAQUE_BIT_7_OPAQUE, 1),
            (Self::PeakDetector, 1) => modify(fields::BBTOP_BYTE_1D_OPAQUE_BITS_4_6_OPAQUE, 4),
            (Self::PeakDetector, 2) => modify(fields::BBTOP_BYTE_03_OPAQUE_BITS_4_6_OPAQUE, 4),
            (Self::Sar2InitializationCode(code), 0) => modify(
                fields::SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR,
                code.0.to_le_bytes()[1],
            ),
            (Self::Sar2InitializationCode(code), 1) => PhyI2cConfigurationCommand::Write(
                registers::SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR,
                code.0 as u8,
            ),
            _ => return None,
        })
    }
}

/// Bus clock selection of `phy_i2c_clk_sel(n)`, for `n` within the SDA
/// side-guard field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cClockSelection(u8);

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cClockSelection {
    pub const fn new(selection: u8) -> Option<Self> {
        if selection <= 31 {
            Some(Self(selection))
        } else {
            None
        }
    }
}

/// A command is still owned by its host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub enum PhyI2cAccessError {
    Busy,
}

/// Unique owner of the analog register I2C master.
#[must_use = "dropping the analog I2C owner loses its register authority"]
#[cfg_attr(
    not(any(test, feature = "test-support", oer_probe)),
    expect(
        dead_code,
        reason = "the product capability reserves the analog I2C registers; only probes drive them"
    )
)]
pub struct PhyI2cRegisters {
    peripherals: svd::peripheral_ownership::PhyI2cPeripherals,
    /// The host map was rewritten for a command that has not started yet
    /// because its host was busy. The map image is the same for every block,
    /// so a retry only polls the host, as the vendor leaves do.
    host_map_pending: bool,
}

impl PhyI2cRegisters {
    pub(crate) const fn new(peripherals: svd::peripheral_ownership::PhyI2cPeripherals) -> Self {
        Self {
            peripherals,
            host_map_pending: false,
        }
    }
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cRegisters {
    /// Rewrite the host map once per command and return the block's host.
    fn select_host_once(&mut self, block: Route) -> Host {
        if self.host_map_pending {
            block.host
        } else {
            self.host_map_pending = true;
            self.configure_and_select_host(block)
        }
    }

    fn master(&self) -> &svd::I2cAnaMst {
        &self.peripherals.i2c_ana_mst
    }

    /// Replace the host-selection field and return the block's host, as
    /// `phy_get_i2c_hostid_` does on every access.
    pub fn configure_and_select_host(&mut self, block: Route) -> Host {
        crate::generated::configure_phy_i2c_host_map(self.master());
        block.host
    }

    /// Install the host map of the parallel initialization sequence.
    pub fn select_parallel_host_map(&mut self) {
        crate::generated::configure_phy_i2c_parallel_host_map(self.master());
    }

    /// Restore the normal host map after a parallel sequence.
    pub fn restore_host_map(&mut self) {
        crate::generated::configure_phy_i2c_host_map(self.master());
    }

    /// Publish both commands of a parallel write, host 0 first, without
    /// waiting for either host.
    pub fn start_parallel_pair(&mut self, pair: PhyI2cParallelWrite) {
        let (address, value) = pair.first;
        svd::zero_based_field_write::publish_phy_i2c_host0_command(
            self.master(),
            address.block(),
            address.index(),
            value,
            true,
            false,
        );
        let (address, value) = pair.second;
        svd::zero_based_field_write::publish_phy_i2c_host1_command(
            self.master(),
            address.block(),
            address.index(),
            value,
            true,
            false,
        );
    }

    /// `phy_i2c_clk_sel(n)`: in the host-0, host-1 and hardware-host timing
    /// words, the SDA side guard becomes `n` and then the SCL pulse duration
    /// `n == 0 ? 1 : 2n`.
    #[cfg(oer_probe)]
    pub fn select_clock(&mut self, selection: PhyI2cClockSelection) {
        use crate::generated::{PhyI2cSclPulseDuration, PhyI2cSdaSideGuard};
        let n = u32::from(selection.0);
        let guard = PhyI2cSdaSideGuard::new(n).expect("a selection fits the guard field");
        let pulse = PhyI2cSclPulseDuration::new(if n == 0 { 1 } else { 2 * n })
            .expect("twice a guard fits the pulse field");
        crate::generated::set_phy_i2c_host0_sda_side_guard(self.master(), guard);
        crate::generated::set_phy_i2c_host0_scl_pulse_duration(self.master(), pulse);
        crate::generated::set_phy_i2c_host1_sda_side_guard(self.master(), guard);
        crate::generated::set_phy_i2c_host1_scl_pulse_duration(self.master(), pulse);
        crate::generated::set_phy_i2c_hardware_host_sda_side_guard(self.master(), guard);
        crate::generated::set_phy_i2c_hardware_host_scl_pulse_duration(self.master(), pulse);
    }

    /// `phy_i2cmst_reg_init`: select PHY register mode two and enable PHY
    /// register access in the master control word.
    ///
    /// SOURCE: reviewed evidence `C5_BLOB_LIBPHY_RF_INIT_LEAVES_2`.
    #[cfg(oer_probe)]
    pub fn initialize_master_registers(&mut self) {
        crate::generated::set_phy_i2c_master_register_mode(self.master());
        crate::generated::enable_phy_i2c_master_registers(self.master());
    }

    /// `phy_bbpll_cal(start)`: select BBPLL calibration mode two when
    /// `start` and one otherwise.
    ///
    /// SOURCE: reviewed evidence `C5_BLOB_LIBPHY_RF_INIT_LEAVES_5`.
    pub fn calibrate_bbpll(&mut self, start: bool) {
        use crate::generated::PhyBbpllCalibrationMode;
        let mode = if start {
            PhyBbpllCalibrationMode::Two
        } else {
            PhyBbpllCalibrationMode::One
        };
        crate::generated::set_phy_i2c_bbpll_calibration_mode(self.master(), mode);
    }

    /// `phy_bbpll_recal`: mode two, one read of the master control word, then
    /// `phy_bbpll_cal(0)`.
    #[cfg(oer_probe)]
    pub fn recalibrate_bbpll(&mut self) {
        self.calibrate_bbpll(true);
        let _ = svd::field_read::observe_phy_i2c_bbpll_calibration_mode(self.master());
        self.calibrate_bbpll(false);
    }

    /// Whether a host is executing a command.
    pub fn is_busy(&self, host: Host) -> bool {
        match host {
            Host::Host0 => svd::field_read::observe_phy_i2c_host0_busy(self.master()),
            Host::Host1 => svd::field_read::observe_phy_i2c_host1_busy(self.master()),
        }
    }

    fn publish(&mut self, host: Host, address: Request, value: u8, write: bool) {
        self.host_map_pending = false;
        let code = address.route().command_code;
        match host {
            Host::Host0 => svd::zero_based_field_write::publish_phy_i2c_host0_command(
                self.master(),
                code,
                address.index(),
                value,
                write,
                true,
            ),
            Host::Host1 => svd::zero_based_field_write::publish_phy_i2c_host1_command(
                self.master(),
                code,
                address.index(),
                value,
                write,
                true,
            ),
        }
    }

    /// Start one read: select the host, publish the block's read mask and
    /// the read command.
    ///
    /// # Errors
    ///
    /// The block's host is busy; the host map has been rewritten once for
    /// this command, and a retry only polls the host.
    pub fn try_start_read(&mut self, address: Request) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.route());
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        svd::zero_based_field_write::publish_phy_i2c_read_mask(
            self.master(),
            address.route().complemented_read_select(),
        );
        self.publish(host, address, 0, false);
        Ok(())
    }

    /// Sample the byte of a completed read of the same address.
    ///
    /// # Errors
    ///
    /// The host is still busy.
    pub fn try_finish_read(&self, address: Request) -> Result<u8, PhyI2cAccessError> {
        let host = address.route().host;
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        Ok(match host {
            Host::Host0 => svd::field_read::observe_phy_i2c_host0_data(self.master()),
            Host::Host1 => svd::field_read::observe_phy_i2c_host1_data(self.master()),
        })
    }

    /// Start one write once the block's host is idle.
    ///
    /// # Errors
    ///
    /// The block's host is busy; the host map has been rewritten once for
    /// this command, and a retry only polls the host.
    pub fn try_start_write(
        &mut self,
        address: Request,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.route());
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish(host, address, value, true);
        Ok(())
    }

    /// Observe the completion of a write of the same address.
    ///
    /// # Errors
    ///
    /// The host is still busy.
    pub fn try_finish_write(&self, address: Request) -> Result<(), PhyI2cAccessError> {
        if self.is_busy(address.route().host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modeled_initialization_payloads_match_the_old_packing() {
        for parameter in 0..=u16::MAX {
            let inputs = PhyI2cInitializationInputs {
                parameter_416: parameter,
                ..Default::default()
            };
            assert_eq!(
                inputs.pair(13).expect("pair").first().1,
                ((parameter >> 2) as u8) | 0x40
            );
            assert_eq!(
                inputs.pair(22).expect("pair").first().1,
                ((parameter << 4) as u8) & 0x30
            );
        }
        for raw in 0..=0xfff {
            let code = PhyI2cSar2Code::new(raw).expect("12-bit SAR code");
            assert!(
                matches!(PhyI2cConfiguration::Sar2InitializationCode(code).command(0),
                Some(PhyI2cConfigurationCommand::Modify { value, .. }) if value == (raw >> 8) as u8)
            );
        }
    }

    #[test]
    fn saturation_follows_phy_get_data_sat() {
        assert_eq!(clamp(9, 10, 50), 10);
        assert_eq!(clamp(10, 10, 50), 10);
        assert_eq!(clamp(50, 10, 50), 50);
        assert_eq!(clamp(51, 10, 50), 50);
        assert_eq!(clamp(0xff, 10, 60), 60);
    }

    #[test]
    fn the_initialization_pairs_carry_their_parameters() {
        let base = PhyI2cInitializationInputs::default();
        let changed = PhyI2cInitializationInputs {
            parameter_f7: 0xff,
            parameter_416: 0x3ff,
            ..base
        };
        let differing: std::vec::Vec<usize> = (0..PHY_I2C_INITIALIZATION_PAIR_COUNT)
            .filter(|&index| base.pair(index) != changed.pair(index))
            .collect();
        // parameter_f7 feeds one raw and one saturated byte; 0x416 feeds two.
        assert_eq!(differing.len(), 4);
        assert!(base.pair(PHY_I2C_INITIALIZATION_PAIR_COUNT).is_none());
        assert!(base.pair(PHY_I2C_INITIALIZATION_PAIR_COUNT - 1).is_some());
        // The saturated byte never leaves 10..=50.
        let saturated = |inputs: PhyI2cInitializationInputs| {
            (0..PHY_I2C_INITIALIZATION_PAIR_COUNT)
                .filter_map(|index| inputs.pair(index))
                .map(|pair| pair.second().1)
                .collect::<std::vec::Vec<_>>()
        };
        let low = saturated(PhyI2cInitializationInputs {
            parameter_f7: 0,
            ..base
        });
        let high = saturated(PhyI2cInitializationInputs {
            parameter_f7: 0xff,
            ..base
        });
        assert!(low.contains(&10) && high.contains(&50));
    }
}
