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
use crate::generated::{PhyI2cField, PhyI2cReadMask, phy_i2c_fields as fields};
use crate::svd;

/// One of the two analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Validated identity of one analog block the libphy leaves accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cBlock {
    code: u8,
}

/// First and last block with a read-mask entry in `phy_get_i2c_read_mask_`.
#[cfg(any(test, feature = "test-support", oer_probe))]
const FIRST_BLOCK: u8 = 0x61;
#[cfg(any(test, feature = "test-support", oer_probe))]
const LAST_BLOCK: u8 = 0x6f;

/// Blocks 0x63 to 0x6E that `phy_get_i2c_hostid_` assigns to host 1, as bit
/// `block - 0x63`.
#[cfg(any(test, feature = "test-support", oer_probe))]
const HOST1_BLOCKS: u16 = 0x09ad;

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cBlock {
    /// Convert the raw block byte of the vendor ABI.
    pub const fn from_vendor_abi(code: u8) -> Option<Self> {
        if code >= FIRST_BLOCK && code <= LAST_BLOCK {
            Some(Self { code })
        } else {
            None
        }
    }

    pub const fn code(self) -> u8 {
        self.code
    }

    /// The host `phy_get_i2c_hostid_` selects for the block as called.
    pub const fn host(self) -> PhyI2cHost {
        let offset = self.code.wrapping_sub(0x63);
        if offset <= 11 && HOST1_BLOCKS & (1 << offset) != 0 {
            PhyI2cHost::Host1
        } else {
            PhyI2cHost::Host0
        }
    }

    /// The complemented one-hot read mask `phy_chip_i2c_readReg_org`
    /// publishes for the block as called.
    pub const fn read_mask(self) -> PhyI2cReadMask {
        match self.code {
            0x61 => PhyI2cReadMask::Block61,
            0x63 => PhyI2cReadMask::Block63,
            0x65 => PhyI2cReadMask::Block65,
            0x66 => PhyI2cReadMask::Block66,
            0x67 => PhyI2cReadMask::Block67,
            0x68 => PhyI2cReadMask::Block68,
            0x69 => PhyI2cReadMask::Block69,
            0x6a => PhyI2cReadMask::Block6A,
            0x6b => PhyI2cReadMask::Block6B,
            0x6c => PhyI2cReadMask::Block6C,
            0x6d => PhyI2cReadMask::Block6D,
            0x6e => PhyI2cReadMask::Block6E,
            0x6f => PhyI2cReadMask::Block6F,
            _ => PhyI2cReadMask::Unselected,
        }
    }

    /// The block code the command carries: both leaves address block 0x65
    /// when called with block 0x62, after choosing its host and read mask.
    pub const fn command_code(self) -> u8 {
        if self.code == 0x62 { 0x65 } else { self.code }
    }
}

/// Validated address of one analog byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cAddress {
    block: PhyI2cBlock,
    register: u8,
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cAddress {
    pub const fn new(block: PhyI2cBlock, register: u8) -> Self {
        Self { block, register }
    }

    pub const fn block(self) -> PhyI2cBlock {
        self.block
    }

    pub const fn register(self) -> u8 {
        self.register
    }
}

/// One command of each host in a parallel write, as `phy_i2c_paral_write`
/// publishes them: host 0 first, then host 1, without the start bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub struct PhyI2cParallelWrite {
    first: (u8, u8, u8),
    second: (u8, u8, u8),
}

#[cfg(any(test, feature = "test-support", oer_probe))]
impl PhyI2cParallelWrite {
    /// The (block, register, data) commands of host 0 and host 1.
    pub const fn new(first: (u8, u8, u8), second: (u8, u8, u8)) -> Self {
        Self { first, second }
    }

    pub const fn first(self) -> (u8, u8, u8) {
        self.first
    }

    pub const fn second(self) -> (u8, u8, u8) {
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
            0 => ((0x6b, 0x2, 0x72), (0x6a, 0x0, 0xff)),
            1 => ((0x6b, 0x3, 0xaa), (0x6a, 0x1, 0x7f)),
            2 => ((0x6b, 0xe, 0x55), (0x67, 0x3, 0x44)),
            3 => ((0x6b, 0x7, 0xff), (0x67, 0x3, 0x44)),
            4 => ((0x6b, 0xa, 0x8), (0x67, 0x2, 0x3)),
            5 => ((0x6b, 0xc, 0x55), (0x67, 0x1, 0x6f)),
            6 => ((0x6b, 0xf, 0x81), (0x67, 0x5, 0x6b)),
            7 => ((0x6b, 0x9, 0x0), (0x67, 0x1d, 0xc2)),
            8 => ((0x6b, 0x5, 0x33), (0x67, 0x6, self.parameter_f5)),
            9 => (
                (0x6b, 0x6, 0x30),
                (0x67, 0x8, clamp(self.parameter_f7, 10, 50)),
            ),
            10 => ((0x6b, 0xd, 0x57), (0x67, 0xa, self.parameter_f5)),
            11 => ((0x6b, 0x8, 0xfd), (0x67, 0xc, self.parameter_f7)),
            12 => ((0x6b, 0x4, 0xac), (0x67, 0x7, self.parameter_f6)),
            13 => (
                (
                    0x6e,
                    0x5,
                    crate::generated::layouts::PhyI2cCommandWord::from_bits(
                        crate::generated::PhyI2cInitializationHighCommand::compose(
                            self.parameter_416,
                        )
                        .get(),
                    )
                    .data(),
                ),
                (0x67, 0x9, clamp(self.parameter_f8, 10, 60)),
            ),
            14 => ((0x6e, 0x7, 0x63), (0x67, 0xb, self.parameter_f6)),
            15 => ((0x6e, 0x8, 0x73), (0x67, 0xd, self.parameter_f8)),
            16 => ((0x6e, 0x9, 0xc), (0x67, 0xe, self.parameter_fb)),
            17 => ((0x6e, 0xd, 0x22), (0x67, 0x10, self.parameter_fb)),
            18 => ((0x6e, 0x1, 0x71), (0x67, 0x12, self.parameter_f9)),
            19 => ((0x6e, 0x10, 0x63), (0x67, 0x14, self.parameter_f9)),
            20 => ((0x6e, 0x11, 0x73), (0x67, 0xf, self.parameter_fc)),
            21 => ((0x6e, 0x4, 0x47), (0x67, 0x11, self.parameter_fc)),
            22 => (
                (
                    0x6e,
                    0xc,
                    crate::generated::layouts::PhyI2cCommandWord::from_bits(
                        crate::generated::PhyI2cInitializationLowCommand::compose(
                            self.parameter_416,
                        )
                        .get(),
                    )
                    .data(),
                ),
                (0x67, 0x13, self.parameter_fa),
            ),
            23 => ((0x6e, 0xf, 0x47), (0x67, 0x15, self.parameter_fa)),
            24 => ((0x6e, 0x12, 0x44), (0x6a, 0x3, 0xf)),
            25 => ((0x6e, 0x13, 0x63), (0x67, 0x3, 0x44)),
            26 => ((0x6e, 0x14, 0x73), (0x67, 0x3, 0x44)),
            27 => ((0x63, 0xf, 0x3f), (0x67, 0x3, 0x44)),
            28 => ((0x63, 0x1, 0xab), (0x67, 0x3, 0x44)),
            29 => ((0x63, 0x13, 0x90), (0x67, 0x3, 0x44)),
            30 => ((0x63, 0x6, 0xe0), (0x67, 0x3, 0x44)),
            31 => ((0x63, 0x15, 0x98), (0x67, 0x3, 0x44)),
            32 => ((0x63, 0x11, 0xc), (0x67, 0x3, 0x44)),
            33 => ((0x63, 0x14, 0xa), (0x67, 0x3, 0x44)),
            34 => ((0x63, 0x0, 0xdb), (0x67, 0x3, 0x44)),
            35 => ((0x63, 0xe, 0x0), (0x67, 0x3, 0x44)),
            36 => ((0x63, 0x8, 0x61), (0x67, 0x3, 0x44)),
            37 => ((0x63, 0x7, 0x0), (0x67, 0x3, 0x44)),
            38 => ((0x6e, 0xa, self.parameter_410), (0x67, 0x3, 0x44)),
            39 => ((0x6e, 0xb, self.parameter_412), (0x67, 0x3, 0x44)),
            40 => ((0x63, 0x1a, 0x11), (0x67, 0x3, 0x44)),
            41 => ((0x63, 0x19, 0x48), (0x67, 0x3, 0x44)),
            42 => ((0x63, 0x12, 0x0), (0x67, 0x3, 0x44)),
            43 => ((0x63, 0x18, 0xdd), (0x67, 0x3, 0x44)),
            _ => return None,
        };
        Some(PhyI2cParallelWrite::new(first, second))
    }
}

/// One command of an analog configuration leaf, in the vendor order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support", oer_probe))]
pub enum PhyI2cConfigurationCommand {
    /// `phy_i2c_writeReg`: write a whole byte.
    Write(PhyI2cAddress, u8),
    /// `phy_i2c_writeReg_Mask(block, host, reg, msb, lsb, value)`.
    Modify {
        address: PhyI2cAddress,
        msb: u8,
        lsb: u8,
        value: u8,
    },
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
        const fn modify(field: PhyI2cField, value: u8) -> PhyI2cConfigurationCommand {
            let mask = field.replace(0, u8::MAX);
            PhyI2cConfigurationCommand::Modify {
                address: PhyI2cAddress::new(PhyI2cBlock { code: field.bank() }, field.register()),
                msb: 7 - mask.leading_zeros() as u8,
                lsb: mask.trailing_zeros() as u8,
                value,
            }
        }
        Some(match (self, index) {
            (Self::Band, 0) => PhyI2cConfigurationCommand::Write(
                PhyI2cAddress::new(PhyI2cBlock { code: 0x6a }, 1),
                0x7f,
            ),
            (Self::CrystalRegisters, 0) => modify(fields::XTAL_REGISTER_8_CONTROL, 0),
            (Self::CrystalRegisters, 1) => modify(fields::XTAL_REGISTER_7_CONTROL, 1),
            (Self::DacRate, 0) => modify(fields::DAC_RATE, 0),
            (Self::AdcRate(rate), 0) => modify(fields::ADC_RATE, if rate { 0 } else { 1 }),
            (Self::BiasRegisters, 0) => modify(fields::BIAS_REGISTER_0_LOW, 0xf),
            (Self::BiasRegisters, 1) => modify(fields::BIAS_REGISTER_1_HIGH, 7),
            (Self::BiasRegisters, 2) => modify(fields::BIAS_REGISTER_0_HIGH, 9),
            (Self::BiasRegisters, 3) => modify(fields::BIAS_REGISTER_1_LOW, 0xf),
            (Self::RcCalibration(arguments), 0) => {
                modify(fields::RC_CALIBRATION_FIRST, arguments.first)
            }
            (Self::RcCalibration(arguments), 1) => {
                modify(fields::RC_CALIBRATION_SECOND, arguments.second)
            }
            (Self::RcCalibration(arguments), 2) => {
                modify(fields::RC_CALIBRATION_THIRD, arguments.third)
            }
            (Self::FilterCapacitors(_), 0) => modify(fields::FILTER_REGISTER_1D_CONTROL, 0),
            (Self::FilterCapacitors(_), 1) => modify(fields::FILTER_REGISTER_5_BIT_6, 1),
            (Self::FilterCapacitors(_), 2) => modify(fields::FILTER_REGISTER_5_BIT_3, 1),
            (Self::FilterCapacitors(_), 3) => modify(fields::FILTER_REGISTER_5_BIT_5, 1),
            (Self::FilterCapacitors(inputs), 4..=19) => {
                let (register, value) = match index {
                    4 => (6, inputs.parameter_f5),
                    5 => (8, clamp(inputs.parameter_f7, 10, 50)),
                    6 => (0xa, inputs.parameter_f5),
                    7 => (0xc, inputs.parameter_f7),
                    8 => (7, inputs.parameter_f6),
                    9 => (9, clamp(inputs.parameter_f8, 10, 60)),
                    10 => (0xb, inputs.parameter_f6),
                    11 => (0xd, inputs.parameter_f8),
                    12 => (0xe, inputs.parameter_fb),
                    13 => (0x10, inputs.parameter_fb),
                    14 => (0x12, inputs.parameter_f9),
                    15 => (0x14, inputs.parameter_f9),
                    16 => (0xf, inputs.parameter_fc),
                    17 => (0x11, inputs.parameter_fc),
                    18 => (0x13, inputs.parameter_fa),
                    _ => (0x15, inputs.parameter_fa),
                };
                PhyI2cConfigurationCommand::Write(
                    PhyI2cAddress::new(PhyI2cBlock { code: 0x67 }, register),
                    value,
                )
            }
            (Self::PeakDetector, 0) => modify(fields::PEAK_DETECTOR_ENABLE, 1),
            (Self::PeakDetector, 1) => modify(fields::PEAK_DETECTOR_REFERENCE, 4),
            (Self::PeakDetector, 2) => modify(fields::PEAK_DETECTOR_REGISTER_3_CONTROL, 4),
            (Self::Sar2InitializationCode(code), 0) => {
                modify(fields::SAR2_CODE_HIGH, code.0.to_le_bytes()[1])
            }
            (Self::Sar2InitializationCode(code), 1) => PhyI2cConfigurationCommand::Write(
                PhyI2cAddress::new(PhyI2cBlock { code: 0x69 }, 3),
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
    fn select_host_once(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        if self.host_map_pending {
            block.host()
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
    pub fn configure_and_select_host(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        crate::generated::configure_phy_i2c_host_map(self.master());
        block.host()
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
        let (block, register, value) = pair.first;
        svd::zero_based_field_write::publish_phy_i2c_host0_command(
            self.master(),
            block,
            register,
            value,
            true,
            false,
        );
        let (block, register, value) = pair.second;
        svd::zero_based_field_write::publish_phy_i2c_host1_command(
            self.master(),
            block,
            register,
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
    pub fn is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => svd::field_read::observe_phy_i2c_host0_busy(self.master()),
            PhyI2cHost::Host1 => svd::field_read::observe_phy_i2c_host1_busy(self.master()),
        }
    }

    fn publish(&mut self, host: PhyI2cHost, address: PhyI2cAddress, value: u8, write: bool) {
        self.host_map_pending = false;
        let code = address.block.command_code();
        match host {
            PhyI2cHost::Host0 => svd::zero_based_field_write::publish_phy_i2c_host0_command(
                self.master(),
                code,
                address.register,
                value,
                write,
                true,
            ),
            PhyI2cHost::Host1 => svd::zero_based_field_write::publish_phy_i2c_host1_command(
                self.master(),
                code,
                address.register,
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
    pub fn try_start_read(&mut self, address: PhyI2cAddress) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.block);
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        crate::generated::publish_phy_i2c_read_mask(self.master(), address.block.read_mask());
        self.publish(host, address, 0, false);
        Ok(())
    }

    /// Sample the byte of a completed read of the same address.
    ///
    /// # Errors
    ///
    /// The host is still busy.
    pub fn try_finish_read(&self, address: PhyI2cAddress) -> Result<u8, PhyI2cAccessError> {
        let host = address.block.host();
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        Ok(match host {
            PhyI2cHost::Host0 => svd::field_read::observe_phy_i2c_host0_data(self.master()),
            PhyI2cHost::Host1 => svd::field_read::observe_phy_i2c_host1_data(self.master()),
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
        address: PhyI2cAddress,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.block);
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
    pub fn try_finish_write(&self, address: PhyI2cAddress) -> Result<(), PhyI2cAccessError> {
        if self.is_busy(address.block.host()) {
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
    fn generated_routes_preserve_the_request_host_mask_and_command_alias() {
        use crate::analog_i2c::{self, Host, Profile};
        for code in FIRST_BLOCK..=LAST_BLOCK {
            let previous = PhyI2cBlock::from_vendor_abi(code).unwrap();
            let route = analog_i2c::route(Profile::LibphyNormal, code).unwrap();
            let host = match previous.host() {
                PhyI2cHost::Host0 => Host::Host0,
                PhyI2cHost::Host1 => Host::Host1,
            };
            assert_eq!(route.host, host);
            assert_eq!(route.command_code, previous.command_code());
            assert_eq!(
                route.complemented_read_select(),
                previous.read_mask() as u32
            );
        }
        let alias = analog_i2c::route(Profile::LibphyNormal, 0x62).unwrap();
        let direct = analog_i2c::route(Profile::LibphyNormal, 0x65).unwrap();
        assert_eq!(alias.address(3), direct.address(3));
        assert_ne!(alias.host, direct.host);
        assert_ne!(alias.read_select, direct.read_select);
        assert!(analog_i2c::READ_SAFE.is_empty());
        assert!(analog_i2c::REGISTERS.iter().all(|r| r.reset.is_none()));
    }

    #[test]
    fn modeled_initialization_payloads_match_the_old_packing() {
        for parameter in 0..=u16::MAX {
            let inputs = PhyI2cInitializationInputs {
                parameter_416: parameter,
                ..Default::default()
            };
            assert_eq!(
                inputs.pair(13).expect("pair").first(),
                (0x6e, 5, ((parameter >> 2) as u8) | 0x40)
            );
            assert_eq!(
                inputs.pair(22).expect("pair").first(),
                (0x6e, 12, ((parameter << 4) as u8) & 0x30)
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

    /// The blocks ESP-IDF's `regi2c_impl.c` names agree with the libphy
    /// tables: the same one-hot read-mask bit and the same host.
    #[test]
    fn the_public_regi2c_blocks_agree_with_the_libphy_tables() {
        // (block, ANA_CONF1 read-mask bit, host 0 selected)
        for (code, mask_bit, selects_host0) in [
            (0x6a, 6, false),
            (0x66, 7, false),
            (0x61, 8, true),
            (0x69, 9, true),
            (0x6d, 10, true),
        ] {
            let block = PhyI2cBlock::from_vendor_abi(code).expect("reviewed block");
            assert_eq!(block.read_mask() as u32, !(1_u32 << mask_bit), "{code:#x}");
            let host = if selects_host0 {
                PhyI2cHost::Host0
            } else {
                PhyI2cHost::Host1
            };
            assert_eq!(block.host(), host, "{code:#x}");
        }
    }

    #[test]
    fn block_0x62_is_addressed_as_0x65_with_its_own_host_and_mask() {
        let block = PhyI2cBlock::from_vendor_abi(0x62).expect("block");
        assert_eq!(block.command_code(), 0x65);
        assert_eq!(block.host(), PhyI2cHost::Host0);
        assert_eq!(block.read_mask(), PhyI2cReadMask::Unselected);
        let target = PhyI2cBlock::from_vendor_abi(0x65).expect("block");
        assert_eq!(target.command_code(), 0x65);
        assert_eq!(target.host(), PhyI2cHost::Host1);
    }

    #[test]
    fn blocks_outside_the_mask_table_are_rejected() {
        assert!(PhyI2cBlock::from_vendor_abi(0x60).is_none());
        assert!(PhyI2cBlock::from_vendor_abi(0x70).is_none());
        assert_eq!(
            PhyI2cBlock::from_vendor_abi(0x6e).map(PhyI2cBlock::host),
            Some(PhyI2cHost::Host1)
        );
        assert_eq!(
            PhyI2cBlock::from_vendor_abi(0x6f).map(PhyI2cBlock::host),
            Some(PhyI2cHost::Host0)
        );
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
                .map(|pair| pair.second().2)
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
