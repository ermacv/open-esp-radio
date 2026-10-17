//! Rust-owned slices of the ESP32-S31 baseband cold initializer.
//!
//! The pinned parent is `libphy.a[phy_init.o]::phy_bb_init`, size `0x18e`.
//! This module is intentionally built from independently completed child
//! transitions. It must not grow a generic "call vendor calibration" action:
//! every child becomes either pure Rust state or an explicit I2C, MMIO,
//! observation, timer, or interrupt edge before the parent can use it.
//!
//! The first complete child is
//! `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem`, size `0x76`, called by the
//! parent with exactly 32 entries. The reference reads
//! `TABLE_MEMORY_INDEX_SOURCE.BASE_INDEX` once, then performs four finite MMIO
//! accesses per entry. It has no callback, allocation, hidden software state,
//! wait, delay, or hardware-dependent exit.

pub use oer_esp32s31_hal::types::PhyGainMemoryEntry;

/// Required pinned `libphy.a` vendor-ABI no-op leaf.
///
/// The ESP32-S31 `set_bb_wdg` body is exactly one `ret` instruction.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn set_bb_wdg() {}

pub const PHY_TX_CFR_ENTRY_COUNT: u8 = 32;
const PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT: u8 = 10;
const PHY_TX_CFR_DATA_PREFIX_VALUE: u32 = 0x0000_0e13;
/// Hardware initialization count/limit used by `phy_reg_init`.
///
/// This is not either generated gain bank's final valid index. Use
/// [`PHY_WIFI_RX_GAIN_LAST_INDEX`] or [`PHY_SHARED_RX_GAIN_LAST_INDEX`] when
/// validating a generated or cached bank.
pub const PHY_RX_TABLE_ENTRY_COUNT: u8 = 0x4f;
pub const PHY_WIFI_RX_GAIN_GENERATED_CAPACITY: usize = 0x55;

// Pinned `libphy.a[phy_rx_gain.o]` `.rodata` and stack initializers used by
// `phy_set_rx_gain_table(0x985, 0)`. Keep these byte-oriented: the ROM
// generator deliberately reads signed adjustment bytes from the latter two
// objects and aligned halfwords from the first two.
const PHY_RX_GAIN_BASE_BANK_0: [u16; 8] = [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f];
const PHY_RX_GAIN_BASE_BANK_1: [u16; 11] = [
    0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
];
const PHY_RX_GAIN_ADVANCE_BANK_0: [i8; 8] = [8, 8, 10, 8, 5, 7, 6, 0];
const PHY_RX_GAIN_ADVANCE_BANK_1: [i8; 11] = [6, 5, 5, 5, 7, 5, 7, 7, 5, 4, 0];
const PHY_RX_GAIN_THRESHOLD_BANK_0: [u8; 8] = [3, 5, 3, 9, 12, 12, 12, 10];
const PHY_RX_GAIN_THRESHOLD_BANK_1: [u8; 11] = [0; 11];
const PHY_RX_GAIN_LOW_FIELD: [u16; 7] = [0, 0x20, 0x30, 0x38, 0x3c, 0x3e, 0x3f];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainBank {
    Wifi,
    Shared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyGeneratedRxGainTable {
    pub words: [u32; PHY_WIFI_RX_GAIN_GENERATED_CAPACITY],
    /// Highest valid generated index, matching the return value of ROM
    /// `phy_gen_rx_gain_table`.
    pub last_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainMemoryParameters {
    pub parameter_002: u8,
    pub wifi_index_dc: [[u16; 2]; 8],
    /// Fine radio DC pairs of `phy_rxdc_fine_cal`.
    pub wifi_fine_dc: [[u16; 2]; crate::rx::gain_calibration::FINE_CODES],
    pub shared_index_dc: [[u16; 2]; 11],
    pub wifi_auxiliary: u16,
}

fn rfrx_gain_index(bank: PhyRxGainBank, encoded_gain: u16) -> usize {
    let bases = match bank {
        PhyRxGainBank::Wifi => &PHY_RX_GAIN_BASE_BANK_0[..],
        PhyRxGainBank::Shared => &PHY_RX_GAIN_BASE_BANK_1[..],
    };
    let mut index = 0;
    while index != bases.len() {
        if bases[index] == encoded_gain {
            return index;
        }
        index += 1;
    }
    bases.len()
}

const fn shared_rx_mixer_digital_gain(index: usize) -> u8 {
    match index {
        9 => 4,
        10 => 7,
        _ => 0,
    }
}

/// Reproduce one complete `phy_wr_rx_gain_mem_new` output record.
///
/// The two hardware-clock/PBus wrappers around the reference loop are
/// sequenced by its caller. This function contains the entire per-entry
/// arithmetic and borrows all calibration inputs from their owning PHY epoch.
pub fn phy_generated_rx_gain_memory_entry(
    parameters: &PhyRxGainMemoryParameters,
    bank: PhyRxGainBank,
    table: &PhyGeneratedRxGainTable,
    index: u8,
) -> PhyGainMemoryEntry {
    let encoded = table.words[index as usize];
    let gain_index = rfrx_gain_index(bank, ((encoded >> 12) & 0xffff) as u16);
    let index_dc = match bank {
        PhyRxGainBank::Wifi => parameters.wifi_index_dc[gain_index],
        PhyRxGainBank::Shared => parameters.shared_index_dc[gain_index],
    };
    let mut selected_bits = 0_u8;
    let mut bit = 4_u8;
    while bit != 10 {
        selected_bits = selected_bits.wrapping_add(((encoded >> bit) & 1) as u8);
        bit += 1;
    }
    // `phy_get_rxbb_dc_new` selects the fine pair of the population of
    // entry bits 4..=9, saturated at the last fine code. The vendor publisher
    // discards it for the shared bank, whose per-gain radio DC values still
    // come from shared_index_dc.
    let last_fine = crate::rx::gain_calibration::FINE_CODES - 1;
    let fine = parameters.wifi_fine_dc[if usize::from(selected_bits) > last_fine {
        last_fine
    } else {
        selected_bits as usize
    }];
    let [dc_i, dc_q] = match bank {
        PhyRxGainBank::Wifi => fine,
        PhyRxGainBank::Shared => [0x100, 0x100],
    };
    let auxiliary = match bank {
        PhyRxGainBank::Wifi => parameters.wifi_auxiliary,
        PhyRxGainBank::Shared => 0,
    };
    let mixer_digital_gain: u8 = match bank {
        PhyRxGainBank::Wifi => 7,
        PhyRxGainBank::Shared => shared_rx_mixer_digital_gain(gain_index),
    };
    let memory_index = match bank {
        PhyRxGainBank::Wifi => index,
        PhyRxGainBank::Shared => index.wrapping_add(0x50),
    };
    PhyGainMemoryEntry::generated_receive(
        memory_index,
        [dc_i, dc_q],
        index_dc,
        auxiliary,
        encoded,
        mixer_digital_gain,
        parameters.parameter_002,
    )
}

/// Pure Rust replacement for complete ROM `phy_gen_rx_gain_table`, size 312.
///
/// The diagnostic argument of the ROM function only prints intermediate
/// values and is intentionally absent. The S31 cold parent uses fixed table
/// objects, so exposing the bank rather than five aliasable raw pointers also
/// makes every input and bound explicit.
pub const fn generate_phy_rx_gain_table(bank: PhyRxGainBank) -> PhyGeneratedRxGainTable {
    let (maximum_gain, entry_count) = match bank {
        PhyRxGainBank::Wifi => (0x1c_u8, PHY_RX_GAIN_BASE_BANK_0.len()),
        PhyRxGainBank::Shared => (0x12_u8, PHY_RX_GAIN_BASE_BANK_1.len()),
    };
    let mut words = [0_u32; PHY_WIFI_RX_GAIN_GENERATED_CAPACITY];
    let mut gain = match bank {
        PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[0],
        PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[0],
    };
    let mut source_index = 0_usize;
    let mut output_index = 0_usize;
    loop {
        let advance = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_ADVANCE_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_ADVANCE_BANK_1[source_index],
        };
        let threshold = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[source_index],
        };
        if gain == threshold.wrapping_add_signed(advance) && source_index < entry_count - 1 {
            loop {
                source_index += 1;
                let next_advance = match bank {
                    PhyRxGainBank::Wifi => PHY_RX_GAIN_ADVANCE_BANK_0[source_index],
                    PhyRxGainBank::Shared => PHY_RX_GAIN_ADVANCE_BANK_1[source_index],
                };
                if next_advance != 0 || source_index >= entry_count - 1 {
                    break;
                }
            }
            gain = match bank {
                PhyRxGainBank::Wifi => PHY_RX_GAIN_THRESHOLD_BANK_0[source_index],
                PhyRxGainBank::Shared => PHY_RX_GAIN_THRESHOLD_BANK_1[source_index],
            };
        }
        let base = match bank {
            PhyRxGainBank::Wifi => PHY_RX_GAIN_BASE_BANK_0[source_index],
            PhyRxGainBank::Shared => PHY_RX_GAIN_BASE_BANK_1[source_index],
        };
        let low = PHY_RX_GAIN_LOW_FIELD[(gain / 6) as usize] & 0x0fff;
        words[output_index] = base as u32 * 0x1000 + low as u32 * 0x10 + (gain % 6) as u32;
        if maximum_gain < gain || output_index + 1 == PHY_WIFI_RX_GAIN_GENERATED_CAPACITY {
            return PhyGeneratedRxGainTable {
                words,
                last_index: output_index as u8,
            };
        }
        output_index += 1;
        gain = gain.wrapping_add(1);
    }
}

/// Highest valid index produced for the Wi-Fi receive-gain bank.
pub const PHY_WIFI_RX_GAIN_LAST_INDEX: u8 =
    generate_phy_rx_gain_table(PhyRxGainBank::Wifi).last_index;

/// Highest valid index produced for the shared Bluetooth/IEEE 802.15.4 bank.
pub const PHY_SHARED_RX_GAIN_LAST_INDEX: u8 =
    generate_phy_rx_gain_table(PhyRxGainBank::Shared).last_index;

/// The two explicit `phy_param` bytes consumed by ROM `phy_reg_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterInitParameters {
    pub parameter_121: u8,
    pub parameter_120: u8,
}

/// Explicit inputs captured before the Rust owner publishes the RX table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxTableInitParameters {
    pub parameter_002: u8,
    pub parameter_121: u8,
}

/// Reproduce one of the 79 fixed-form `phy_rx_table_init` entries.
pub const fn phy_rx_table_gain_entry(
    parameters: PhyRxTableInitParameters,
    index: u8,
) -> PhyGainMemoryEntry {
    PhyGainMemoryEntry::receive_table(index, parameters.parameter_002)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrEntry {
    pub index: u8,
    pub start_index: u8,
    pub data: u32,
}

impl PhyTxCfrEntry {
    pub const fn memory_index(self) -> u8 {
        self.start_index.wrapping_add(self.index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrOutcome {
    pub entries_written: u8,
    pub start_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrAction {
    ReadStartIndex,
    ProgramEntry(PhyTxCfrEntry),
    Complete(PhyTxCfrOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrCompletion {
    StartIndexRead { base_index: u8 },
    EntryProgrammed(PhyTxCfrEntry),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxCfrStep {
    ReadStartIndex,
    Entries { start_index: u8, index: u8 },
    Complete(PhyTxCfrOutcome),
}

/// Caller-driven replacement for the exact `phy_set_tx_cfr_mem(32)` child.
///
/// One call to [`Self::advance`] consumes one externally supplied completion.
/// The transition never samples hardware itself and never asks to be polled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCfrTransition {
    step: PhyTxCfrStep,
}

impl PhyTxCfrTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyTxCfrStep::ReadStartIndex,
        }
    }

    pub const fn action(self) -> PhyTxCfrAction {
        match self.step {
            PhyTxCfrStep::ReadStartIndex => PhyTxCfrAction::ReadStartIndex,
            PhyTxCfrStep::Entries { start_index, index } => {
                PhyTxCfrAction::ProgramEntry(PhyTxCfrEntry {
                    index,
                    start_index,
                    data: if index < PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT {
                        PHY_TX_CFR_DATA_PREFIX_VALUE
                    } else {
                        0
                    },
                })
            }
            PhyTxCfrStep::Complete(outcome) => PhyTxCfrAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCfrCompletion,
    ) -> Result<(), PhyTxCfrTransitionError> {
        self.step = match (self.step, completion) {
            (PhyTxCfrStep::ReadStartIndex, PhyTxCfrCompletion::StartIndexRead { base_index }) => {
                PhyTxCfrStep::Entries {
                    start_index: base_index,
                    index: 0,
                }
            }
            (
                PhyTxCfrStep::Entries { start_index, index },
                PhyTxCfrCompletion::EntryProgrammed(completed),
            ) if completed
                == (PhyTxCfrEntry {
                    index,
                    start_index,
                    data: if index < PHY_TX_CFR_DATA_PREFIX_ENTRY_COUNT {
                        PHY_TX_CFR_DATA_PREFIX_VALUE
                    } else {
                        0
                    },
                }) =>
            {
                let next = index + 1;
                if next == PHY_TX_CFR_ENTRY_COUNT {
                    PhyTxCfrStep::Complete(PhyTxCfrOutcome {
                        entries_written: next,
                        start_index,
                    })
                } else {
                    PhyTxCfrStep::Entries {
                        start_index,
                        index: next,
                    }
                }
            }
            (PhyTxCfrStep::Complete(_), _) => return Err(PhyTxCfrTransitionError::AlreadyComplete),
            _ => return Err(PhyTxCfrTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyTxCfrTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCfrBindingError {
    TerminalAction,
}

/// A non-cloneable identity token for one finite TX-CFR MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCfrMmioBinding {
    action: PhyTxCfrAction,
}

impl PhyTxCfrMmioBinding {
    pub fn new(action: PhyTxCfrAction) -> Result<Self, PhyTxCfrBindingError> {
        match action {
            PhyTxCfrAction::ReadStartIndex | PhyTxCfrAction::ProgramEntry(_) => Ok(Self { action }),
            PhyTxCfrAction::Complete(_) => Err(PhyTxCfrBindingError::TerminalAction),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyTxCfrAction {
        self.action
    }

    /// Execute exactly one finite target transaction and consume its token.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxCfrCompletion {
        match self.action {
            PhyTxCfrAction::ReadStartIndex => PhyTxCfrCompletion::StartIndexRead {
                base_index: oer_esp32s31_hal::phy::memory::read_table_memory_base_index(registers),
            },
            PhyTxCfrAction::ProgramEntry(entry) => {
                oer_esp32s31_hal::phy::memory::program_tx_cfr_entry(
                    registers,
                    entry.data,
                    entry.memory_index(),
                );
                PhyTxCfrCompletion::EntryProgrammed(entry)
            }
            PhyTxCfrAction::Complete(_) => unreachable!(),
        }
    }
}

/// Complete parent or child operations already proven to be finite MMIO.
///
/// This enum deliberately excludes every still-unported calibration child.
/// Adding a variant requires the complete body and all of its callees to have
/// been recovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbBasebandMode {
    Idle,
    Calibration,
}

impl PhyBbBasebandMode {
    #[cfg(target_arch = "riscv32")]
    const fn register_value(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Calibration => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfRxSaturationPhase {
    PrepareCheck,
    Finalize,
}

impl PhyRfRxSaturationPhase {
    #[cfg(target_arch = "riscv32")]
    const fn enabled(self) -> bool {
        match self {
            Self::PrepareCheck => false,
            Self::Finalize => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbMmioAction {
    SetForcedDigitalGain {
        enabled: bool,
    },
    EnableBasebandInitialization,
    SetBasebandMode {
        mode: PhyBbBasebandMode,
    },
    UpdateAgcRegisters,
    UpdatePostInitRegisters,
    EnableAgc,
    SetWifiEnabled {
        enabled: bool,
    },
    ConfigureTxPowerTracking {
        enabled: bool,
    },
    ConfigureRfRxSaturation {
        phase: PhyRfRxSaturationPhase,
    },
    ConfigureI2cTxRate,
    ConfigurePhyRegisters {
        parameters: PhyRegisterInitParameters,
    },
    ConfigureRxTable {
        parameters: PhyRxTableInitParameters,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBbMmioCompletion {
    pub action: PhyBbMmioAction,
}

/// A non-cloneable token for one recovered baseband MMIO transaction.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyBbMmioBinding {
    action: PhyBbMmioAction,
}

impl PhyBbMmioBinding {
    pub const fn new(action: PhyBbMmioAction) -> Self {
        Self { action }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyBbMmioAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::PhyInitializationAccess,
    ) -> PhyBbMmioCompletion {
        match self.action {
            PhyBbMmioAction::SetForcedDigitalGain { enabled } => {
                oer_esp32s31_hal::phy::baseband::configure_forced_digital_gain(
                    registers, enabled, -120, -120,
                );
            }
            PhyBbMmioAction::EnableBasebandInitialization => {
                registers.set_phy_calibration_clock(true)
            }
            PhyBbMmioAction::SetBasebandMode { mode } => {
                oer_esp32s31_hal::phy::frequency::set_baseband_mode(
                    registers,
                    mode.register_value(),
                )
            }
            PhyBbMmioAction::UpdateAgcRegisters => {
                crate::hardware::configure_phy_bb_agc_register_update(registers)
            }
            PhyBbMmioAction::UpdatePostInitRegisters => {
                oer_esp32s31_hal::phy::agc::update_post_initialization(registers)
            }
            PhyBbMmioAction::EnableAgc => crate::hardware::enable_phy_agc(registers),
            PhyBbMmioAction::SetWifiEnabled { enabled } => {
                oer_esp32s31_hal::phy::frequency::set_wifi_enabled(registers, enabled)
            }
            PhyBbMmioAction::ConfigureTxPowerTracking { enabled } => {
                oer_esp32s31_hal::phy::baseband::configure_tx_power_tracking(registers, enabled)
            }
            PhyBbMmioAction::ConfigureRfRxSaturation { phase } => {
                oer_esp32s31_hal::phy::agc::configure_rf_rx_saturation(registers, phase.enabled())
            }
            PhyBbMmioAction::ConfigureI2cTxRate => {
                oer_esp32s31_hal::phy::baseband::configure_i2c_tx_rate(registers)
            }
            PhyBbMmioAction::ConfigurePhyRegisters { parameters } => {
                crate::hardware::configure_phy_registers(registers, parameters)
            }
            PhyBbMmioAction::ConfigureRxTable { parameters } => {
                crate::hardware::configure_phy_rx_table(registers, parameters)
            }
        }
        PhyBbMmioCompletion {
            action: self.action,
        }
    }
}

/// Successful completion of the complete Wi-Fi portion of
/// `libphy.a[phy_init.o]::phy_bb_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBbInitOutcome {
    pub calibration_performed: bool,
}

/// Terminal failures from the owned children of `phy_bb_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitFailure {
    TxDc(crate::tx::dc_offset::PhyTxDcFailure),
    Pwdet(crate::tx::power_detector::PhyPwdetFailure),
    TxCap(crate::tx::calibration::PhyTxCapFailure),
    Temperature {
        pass: u8,
        failure: crate::analog::temperature::PhyTemperatureFailure,
    },
    TxPower(crate::tx::power::PhyTxPowerFailure),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetFailure),
    Dcode(crate::analog::dcode::PhyDcodeFailure),
    TxIq(crate::tx::iq::PhyTxIqInitFailure),
    BluetoothTxGain(crate::calibration::bluetooth::PhyBluetoothTxGainInitFailure),
    RxIq(crate::rx::iq::PhyRxIqInitFailure),
    RxSaturation(crate::rx::saturation::PhyRxSaturationOutcome),
    RxGain(crate::rx::gain::PhyRxGainInitFailure),
    Channel(crate::channel::PhyChipChannelFailure),
}

/// Exactly one externally completed operation emitted by the baseband
/// parent. No variant means "call a vendor child".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitAction {
    Mmio(PhyBbMmioAction),
    TxDc(crate::tx::dc_offset::PhyTxDcAction),
    Pwdet(crate::tx::power_detector::PhyPwdetAction),
    TxCap(crate::tx::calibration::PhyTxCapAction),
    Temperature(crate::analog::temperature::PhyTemperatureAction),
    TxPower(crate::tx::power::PhyTxPowerAction),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetAction),
    Dcode(crate::analog::dcode::PhyDcodeAction),
    TxIq(crate::tx::iq::PhyTxIqInitAction),
    TxCfr(PhyTxCfrAction),
    BluetoothTxGain(crate::calibration::bluetooth::PhyBluetoothTxGainInitAction),
    PbusMemory(crate::analog::pbus::memory::PhyPbusMemoryAction),
    RxIq(crate::rx::iq::PhyRxIqInitAction),
    RxSaturation(crate::rx::saturation::PhyRxSaturationAction),
    RxGain(crate::rx::gain::PhyRxGainInitAction),
    Channel(crate::channel::PhyChipChannelAction),
}

/// Identity-bound completion for one [`PhyBbInitAction`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitCompletion {
    Mmio(PhyBbMmioCompletion),
    TxDc(crate::tx::dc_offset::PhyTxDcCompletion),
    Pwdet(crate::tx::power_detector::PhyPwdetCompletion),
    TxCap(crate::tx::calibration::PhyTxCapCompletion),
    Temperature(crate::analog::temperature::PhyTemperatureCompletion),
    TxPower(crate::tx::power::PhyTxPowerCompletion),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetCompletion),
    Dcode(crate::analog::dcode::PhyDcodeCompletion),
    TxIq(crate::tx::iq::PhyTxIqInitCompletion),
    TxCfr(PhyTxCfrCompletion),
    BluetoothTxGain(crate::calibration::bluetooth::PhyBluetoothTxGainInitCompletion),
    PbusMemory(crate::analog::pbus::memory::PhyPbusMemoryCompletion),
    RxIq(crate::rx::iq::PhyRxIqInitCompletion),
    RxSaturation(crate::rx::saturation::PhyRxSaturationCompletion),
    RxGain(crate::rx::gain::PhyRxGainInitCompletion),
    Channel(crate::channel::PhyChipChannelCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbExternalBindingError {
    UnsupportedAction,
}

/// Exhaustive lowering of every non-terminal Wi-Fi `phy_bb_init` action.
///
/// This is the boundary intended for the source-only radio executor. It owns
/// no child state and exposes no vendor callback: each variant is a consumed
/// token for one finite MMIO command, one readiness observation, one PBus
/// transaction, or one externally scheduled Rust timer.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyBbExternalBinding {
    Mmio(PhyBbMmioBinding),
    TxDc(crate::tx::dc_offset::PhyTxDcExternalBinding),
    Pwdet(crate::tx::power_detector::PhyPwdetExternalBinding),
    TxCap(crate::tx::calibration::PhyTxCapExternalBinding),
    Temperature(crate::analog::temperature::PhyTemperatureExternalBinding),
    TxPower(crate::tx::power::PhyTxPowerExternalBinding),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding),
    Dcode(crate::analog::dcode::PhyDcodeExternalBinding),
    TxIq(crate::tx::iq::PhyTxIqInitExternalBinding),
    TxCfr(PhyTxCfrMmioBinding),
    BluetoothTxGain(crate::calibration::bluetooth::PhyBluetoothTxGainInitExternalBinding),
    PbusMemory(crate::analog::pbus::memory::PhyPbusMemoryMmioBinding),
    RxIq(crate::rx::iq::PhyRxIqInitExternalBinding),
    RxSaturation(crate::rx::saturation::PhyRxSaturationExternalBinding),
    RxGain(crate::rx::gain::PhyRxGainInitExternalBinding),
    Channel(crate::channel::PhyChipChannelExternalBinding),
}

impl PhyBbExternalBinding {
    pub fn lower(action: PhyBbInitAction) -> Result<Self, PhyBbExternalBindingError> {
        match action {
            PhyBbInitAction::Mmio(action) => Ok(Self::Mmio(PhyBbMmioBinding::new(action))),
            PhyBbInitAction::TxDc(action) => {
                crate::tx::dc_offset::PhyTxDcExternalBinding::lower(action)
                    .map(Self::TxDc)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Pwdet(action) => {
                crate::tx::power_detector::PhyPwdetExternalBinding::lower(action)
                    .map(Self::Pwdet)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxCap(action) => {
                crate::tx::calibration::PhyTxCapExternalBinding::lower(action)
                    .map(Self::TxCap)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Temperature(action) => {
                crate::analog::temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxPower(action) => {
                crate::tx::power::PhyTxPowerExternalBinding::lower(action)
                    .map(Self::TxPower)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxDcPwdet(action) => {
                crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding::lower(action)
                    .map(Self::TxDcPwdet)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Dcode(action) => {
                crate::analog::dcode::PhyDcodeExternalBinding::lower(action)
                    .map(Self::Dcode)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxIq(action) => {
                crate::tx::iq::PhyTxIqInitExternalBinding::lower(action)
                    .map(Self::TxIq)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::TxCfr(action) => PhyTxCfrMmioBinding::new(action)
                .map(Self::TxCfr)
                .map_err(|_| PhyBbExternalBindingError::UnsupportedAction),
            PhyBbInitAction::BluetoothTxGain(action) => {
                crate::calibration::bluetooth::PhyBluetoothTxGainInitExternalBinding::lower(action)
                    .map(Self::BluetoothTxGain)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::PbusMemory(action) => {
                crate::analog::pbus::memory::PhyPbusMemoryMmioBinding::new(action)
                    .map(Self::PbusMemory)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxIq(action) => {
                crate::rx::iq::PhyRxIqInitExternalBinding::lower(action)
                    .map(Self::RxIq)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxSaturation(action) => {
                crate::rx::saturation::PhyRxSaturationExternalBinding::lower(action)
                    .map(Self::RxSaturation)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::RxGain(action) => {
                crate::rx::gain::PhyRxGainInitExternalBinding::lower(action)
                    .map(Self::RxGain)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
            PhyBbInitAction::Channel(action) => {
                crate::channel::PhyChipChannelExternalBinding::lower(action)
                    .map(Self::Channel)
                    .map_err(|_| PhyBbExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free PHY transition owns exactly one recovered child phase"
)]
enum PhyBbInitStep {
    EnableInitialization,
    SetCalibrationMode,
    EnableForcedDigitalGain,
    DisableForcedDigitalGain,
    FailureDisableForcedDigitalGain(PhyBbInitFailure),
    TxDc(crate::tx::dc_offset::PhyTxDcTransition),
    Pwdet(crate::tx::power_detector::PhyPwdetTransition),
    TxCap(crate::tx::calibration::PhyTxCapTransition),
    TemperatureFirst(crate::analog::temperature::PhyTemperatureTransition),
    TxPower(crate::tx::power::PhyTxPowerTransition),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetTransition),
    Dcode(crate::analog::dcode::PhyDcodeTransition),
    TxIq(crate::tx::iq::PhyTxIqInitTransition),
    TxCfr(PhyTxCfrTransition),
    BluetoothTxGain(crate::calibration::bluetooth::PhyBluetoothTxGainInitTransition),
    PbusMemory(crate::analog::pbus::memory::PhyPbusMemoryTransition),
    TemperatureSecond(crate::analog::temperature::PhyTemperatureTransition),
    RxIq(crate::rx::iq::PhyRxIqInitTransition),
    RxTable(PhyRxTableInitParameters),
    RxSaturationPrepare,
    RxSaturation(crate::rx::saturation::PhyRxSaturationTransition),
    RxGain(crate::rx::gain::PhyRxGainInitTransition),
    RxSaturationFinalize,
    RegisterInit(PhyRegisterInitParameters),
    UpdateAgc,
    UpdatePostInit,
    EnableAgc,
    Channel(crate::channel::PhyChipChannelTransition),
    SetIdleMode,
    DisableWifi,
    ConfigureI2cTxRate,
    ConfigureTxPowerTracking,
    FailureSetIdle(PhyBbInitFailure),
    FailureEnableAgc(PhyBbInitFailure),
    Complete(PhyBbInitOutcome),
    Failed(PhyBbInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBbInitLocalStep {
    /// One state-only terminal child result was committed. The caller may
    /// invoke `step_local` again or yield to another executor task.
    StateAdvanced,
    External(PhyBbInitAction),
    Complete(PhyBbInitOutcome),
    Failed(PhyBbInitFailure),
}

/// Unique Rust owner and exact parent sequencing for the Wi-Fi branch of
/// `phy_bb_init`.
///
/// `step_local` performs at most one local state transition. Every hardware
/// access, timer delay and readiness sample is returned to the outer radio
/// executor as one action. The type has no allocator, waker, retry loop,
/// pointer to `phy_param`, or ROM callback table.
pub struct PhyBbInitTransition {
    state: crate::state::PhyState,
    channel_or_frequency: u16,
    step: PhyBbInitStep,
    calibration_performed: bool,
}

impl PhyBbInitTransition {
    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn new(state: crate::state::PhyState) -> Self {
        Self::new_on_channel(state, 11)
    }

    pub const fn new_on_channel(state: crate::state::PhyState, channel_or_frequency: u16) -> Self {
        Self {
            state,
            channel_or_frequency,
            step: PhyBbInitStep::EnableInitialization,
            calibration_performed: false,
        }
    }

    pub const fn state(&self) -> &crate::state::PhyState {
        &self.state
    }

    pub fn into_state(self) -> crate::state::PhyState {
        self.state
    }

    fn begin_failure(&mut self, failure: PhyBbInitFailure) {
        self.step = PhyBbInitStep::FailureDisableForcedDigitalGain(failure);
    }

    /// `phy_bb_init` runs inside the registration's force-TX/RX pair, so its
    /// channel programming is nested one level deep.
    fn channel_transition(&self) -> crate::channel::PhyChipChannelTransition {
        crate::channel::PhyChipChannelTransition::at_depth(
            crate::channel::PhyChipChannelRequest {
                channel_or_frequency: self.channel_or_frequency,
                cbw: 0,
                parameters: self.state.channel_parameters(),
            },
            crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST.nested(),
        )
    }

    pub fn step_local(&mut self) -> Result<PhyBbInitLocalStep, PhyBbInitTransitionError> {
        let local = match self.step {
            PhyBbInitStep::TxDc(transition) => match transition.action() {
                crate::tx::dc_offset::PhyTxDcAction::Complete(outcome) => {
                    self.state.apply_tx_dc_outcome(outcome);
                    self.step =
                        PhyBbInitStep::Pwdet(crate::tx::power_detector::PhyPwdetTransition::new(
                            self.state.pwdet_parameters(),
                        ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::dc_offset::PhyTxDcAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxDc(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxDc(action)),
            },
            PhyBbInitStep::Pwdet(transition) => match transition.action() {
                crate::tx::power_detector::PhyPwdetAction::Complete(outcome) => {
                    self.state.apply_pwdet_outcome(outcome);
                    self.step =
                        PhyBbInitStep::TxCap(crate::tx::calibration::PhyTxCapTransition::new(
                            self.state.tx_cap_parameters(),
                        ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::power_detector::PhyPwdetAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Pwdet(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Pwdet(action)),
            },
            PhyBbInitStep::TxCap(transition) => match transition.action() {
                crate::tx::calibration::PhyTxCapAction::Complete(outcome) => {
                    self.state.apply_tx_cap_outcome(outcome);
                    self.step = PhyBbInitStep::TemperatureFirst(
                        crate::analog::temperature::PhyTemperatureTransition::new(),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::calibration::PhyTxCapAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxCap(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxCap(action)),
            },
            PhyBbInitStep::TemperatureFirst(transition) => match transition.action() {
                crate::analog::temperature::PhyTemperatureAction::Complete(outcome) => {
                    self.state.apply_temperature_outcome(outcome);
                    self.step =
                        PhyBbInitStep::TxPower(crate::tx::power::PhyTxPowerTransition::new(
                            self.state.tx_power_parameters(),
                        ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::analog::temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Temperature { pass: 1, failure });
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Temperature(action)),
            },
            PhyBbInitStep::TxPower(transition) => match transition.action() {
                crate::tx::power::PhyTxPowerAction::Complete(outcome) => {
                    self.state.apply_tx_power_outcome(outcome);
                    self.step = PhyBbInitStep::TxDcPwdet(
                        crate::tx::dc_power_detector::PhyTxDcPwdetTransition::new(
                            self.state.tx_dc_pwdet_parameters(),
                        ),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::power::PhyTxPowerAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxPower(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxPower(action)),
            },
            PhyBbInitStep::TxDcPwdet(transition) => match transition.action() {
                crate::tx::dc_power_detector::PhyTxDcPwdetAction::Complete(outcome) => {
                    self.state.apply_tx_dc_pwdet_outcome(outcome);
                    self.step =
                        PhyBbInitStep::Dcode(crate::analog::dcode::PhyDcodeTransition::new(
                            self.state.dcode_parameters(),
                        ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::dc_power_detector::PhyTxDcPwdetAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxDcPwdet(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxDcPwdet(action)),
            },
            PhyBbInitStep::Dcode(transition) => match transition.action() {
                crate::analog::dcode::PhyDcodeAction::Complete(outcome) => {
                    self.state.apply_dcode_outcome(outcome);
                    self.step = PhyBbInitStep::TxIq(crate::tx::iq::PhyTxIqInitTransition::new(
                        self.state.tx_iq_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::analog::dcode::PhyDcodeAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Dcode(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Dcode(action)),
            },
            PhyBbInitStep::TxIq(transition) => match transition.action() {
                crate::tx::iq::PhyTxIqInitAction::Complete(outcome) => {
                    self.state.apply_tx_iq_outcome(outcome);
                    self.state.mark_baseband_calibration_complete();
                    self.calibration_performed = true;
                    self.step = PhyBbInitStep::TxCfr(PhyTxCfrTransition::new());
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::tx::iq::PhyTxIqInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::TxIq(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxIq(action)),
            },
            PhyBbInitStep::TxCfr(transition) => match transition.action() {
                PhyTxCfrAction::Complete(_) => {
                    self.step = PhyBbInitStep::BluetoothTxGain(
                        // Nested in the registration's force-TX/RX pair.
                        self.state.bluetooth_tx_gain_init_transition(
                            crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST.nested(),
                        ),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::TxCfr(action)),
            },
            PhyBbInitStep::BluetoothTxGain(mut transition) => match transition
                .step_local()
                .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?
            {
                crate::calibration::bluetooth::PhyBluetoothTxGainInitLocalStep::StateAdvanced => {
                    self.step = PhyBbInitStep::BluetoothTxGain(transition);
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::calibration::bluetooth::PhyBluetoothTxGainInitLocalStep::External(
                    action,
                ) => {
                    self.step = PhyBbInitStep::BluetoothTxGain(transition);
                    PhyBbInitLocalStep::External(PhyBbInitAction::BluetoothTxGain(action))
                }
                crate::calibration::bluetooth::PhyBluetoothTxGainInitLocalStep::Complete(
                    outcome,
                ) => {
                    self.state.apply_bluetooth_tx_gain_init_outcome(outcome);
                    self.step = PhyBbInitStep::PbusMemory(
                        crate::analog::pbus::memory::PhyPbusMemoryTransition::new(
                            self.state.pbus_memory_parameters(),
                        ),
                    );
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::calibration::bluetooth::PhyBluetoothTxGainInitLocalStep::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::BluetoothTxGain(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
            },
            PhyBbInitStep::PbusMemory(transition) => match transition.action() {
                crate::analog::pbus::memory::PhyPbusMemoryAction::Complete(_) => {
                    self.step = PhyBbInitStep::DisableForcedDigitalGain;
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::PbusMemory(action)),
            },
            PhyBbInitStep::TemperatureSecond(transition) => match transition.action() {
                crate::analog::temperature::PhyTemperatureAction::Complete(outcome) => {
                    self.state.apply_temperature_outcome(outcome);
                    self.step = PhyBbInitStep::RxIq(crate::rx::iq::PhyRxIqInitTransition::new(
                        self.state.rx_iq_parameters(),
                    ));
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::analog::temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Temperature { pass: 2, failure });
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Temperature(action)),
            },
            PhyBbInitStep::RxIq(transition) => match transition.action() {
                crate::rx::iq::PhyRxIqInitAction::Complete(outcome) => {
                    self.state.apply_rx_iq_outcome(outcome);
                    let parameters = self.state.prepare_rx_table_init();
                    self.step = PhyBbInitStep::RxTable(parameters);
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::rx::iq::PhyRxIqInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::RxIq(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxIq(action)),
            },
            PhyBbInitStep::RxSaturation(transition) => match transition.action() {
                crate::rx::saturation::PhyRxSaturationAction::Complete(outcome) => {
                    if self.state.apply_rx_saturation_outcome(outcome).is_ok() {
                        self.step =
                            PhyBbInitStep::RxGain(crate::rx::gain::PhyRxGainInitTransition::new(
                                self.state.rx_gain_init_parameters(),
                            ));
                    } else {
                        self.begin_failure(PhyBbInitFailure::RxSaturation(outcome));
                    }
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxSaturation(action)),
            },
            PhyBbInitStep::RxGain(transition) => match transition.action() {
                crate::rx::gain::PhyRxGainInitAction::Complete => {
                    let Some(Ok(outcome)) = transition.terminal() else {
                        unreachable!("complete RX gain action must retain a successful outcome")
                    };
                    self.state.apply_rx_gain_init_outcome(outcome);
                    self.step = PhyBbInitStep::RxSaturationFinalize;
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::rx::gain::PhyRxGainInitAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::RxGain(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::RxGain(action)),
            },
            PhyBbInitStep::Channel(transition) => match transition.action() {
                crate::channel::PhyChipChannelAction::Complete(outcome) => {
                    self.state.apply_channel_outcome(outcome);
                    self.step = PhyBbInitStep::SetIdleMode;
                    PhyBbInitLocalStep::StateAdvanced
                }
                crate::channel::PhyChipChannelAction::Failed(failure) => {
                    self.begin_failure(PhyBbInitFailure::Channel(failure));
                    PhyBbInitLocalStep::StateAdvanced
                }
                action => PhyBbInitLocalStep::External(PhyBbInitAction::Channel(action)),
            },
            PhyBbInitStep::Complete(outcome) => PhyBbInitLocalStep::Complete(outcome),
            PhyBbInitStep::Failed(failure) => PhyBbInitLocalStep::Failed(failure),
            step => PhyBbInitLocalStep::External(PhyBbInitAction::Mmio(step.mmio_action())),
        };
        Ok(local)
    }

    pub fn advance_external(
        &mut self,
        completion: PhyBbInitCompletion,
    ) -> Result<(), PhyBbInitTransitionError> {
        let step = match (self.step, completion) {
            (PhyBbInitStep::EnableInitialization, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableBasebandInitialization =>
            {
                PhyBbInitStep::SetCalibrationMode
            }
            (PhyBbInitStep::SetCalibrationMode, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Calibration,
                    }) =>
            {
                PhyBbInitStep::EnableForcedDigitalGain
            }
            (PhyBbInitStep::EnableForcedDigitalGain, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetForcedDigitalGain { enabled: true }) =>
            {
                if self.state.baseband_calibration_complete() {
                    PhyBbInitStep::TxCfr(PhyTxCfrTransition::new())
                } else {
                    PhyBbInitStep::TxDc(crate::tx::dc_offset::PhyTxDcTransition::new(
                        self.state.tx_dc_parameters(),
                    ))
                }
            }
            (PhyBbInitStep::TxDc(mut transition), PhyBbInitCompletion::TxDc(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxDc(transition)
            }
            (PhyBbInitStep::Pwdet(mut transition), PhyBbInitCompletion::Pwdet(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Pwdet(transition)
            }
            (PhyBbInitStep::TxCap(mut transition), PhyBbInitCompletion::TxCap(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxCap(transition)
            }
            (
                PhyBbInitStep::TemperatureFirst(mut transition),
                PhyBbInitCompletion::Temperature(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TemperatureFirst(transition)
            }
            (PhyBbInitStep::TxPower(mut transition), PhyBbInitCompletion::TxPower(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxPower(transition)
            }
            (
                PhyBbInitStep::TxDcPwdet(mut transition),
                PhyBbInitCompletion::TxDcPwdet(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxDcPwdet(transition)
            }
            (PhyBbInitStep::Dcode(mut transition), PhyBbInitCompletion::Dcode(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Dcode(transition)
            }
            (PhyBbInitStep::TxIq(mut transition), PhyBbInitCompletion::TxIq(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxIq(transition)
            }
            (PhyBbInitStep::TxCfr(mut transition), PhyBbInitCompletion::TxCfr(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TxCfr(transition)
            }
            (
                PhyBbInitStep::BluetoothTxGain(mut transition),
                PhyBbInitCompletion::BluetoothTxGain(completed),
            ) => {
                transition
                    .advance_external(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::BluetoothTxGain(transition)
            }
            (
                PhyBbInitStep::PbusMemory(mut transition),
                PhyBbInitCompletion::PbusMemory(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::PbusMemory(transition)
            }
            (
                PhyBbInitStep::TemperatureSecond(mut transition),
                PhyBbInitCompletion::Temperature(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::TemperatureSecond(transition)
            }
            (PhyBbInitStep::RxIq(mut transition), PhyBbInitCompletion::RxIq(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxIq(transition)
            }
            (PhyBbInitStep::RxTable(parameters), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigureRxTable { parameters } =>
            {
                PhyBbInitStep::RxSaturationPrepare
            }
            (PhyBbInitStep::RxSaturationPrepare, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureRfRxSaturation {
                        phase: PhyRfRxSaturationPhase::PrepareCheck,
                    }) =>
            {
                PhyBbInitStep::RxSaturation(crate::rx::saturation::PhyRxSaturationTransition::new(
                    self.state.rx_saturation_parameter_002(),
                ))
            }
            (
                PhyBbInitStep::RxSaturation(mut transition),
                PhyBbInitCompletion::RxSaturation(completed),
            ) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxSaturation(transition)
            }
            (PhyBbInitStep::RxGain(mut transition), PhyBbInitCompletion::RxGain(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::RxGain(transition)
            }
            (PhyBbInitStep::RxSaturationFinalize, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureRfRxSaturation {
                        phase: PhyRfRxSaturationPhase::Finalize,
                    }) =>
            {
                PhyBbInitStep::RegisterInit(self.state.register_init_parameters())
            }
            (PhyBbInitStep::RegisterInit(parameters), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigurePhyRegisters { parameters } =>
            {
                PhyBbInitStep::UpdateAgc
            }
            (PhyBbInitStep::UpdateAgc, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::UpdateAgcRegisters =>
            {
                PhyBbInitStep::UpdatePostInit
            }
            (PhyBbInitStep::UpdatePostInit, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::UpdatePostInitRegisters =>
            {
                PhyBbInitStep::EnableAgc
            }
            (PhyBbInitStep::EnableAgc, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableAgc =>
            {
                PhyBbInitStep::Channel(self.channel_transition())
            }
            (PhyBbInitStep::Channel(mut transition), PhyBbInitCompletion::Channel(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyBbInitTransitionError::WrongCompletion)?;
                PhyBbInitStep::Channel(transition)
            }
            (PhyBbInitStep::SetIdleMode, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Idle,
                    }) =>
            {
                if self.state.disable_wifi_after_baseband_init() {
                    PhyBbInitStep::DisableWifi
                } else {
                    PhyBbInitStep::ConfigureI2cTxRate
                }
            }
            (PhyBbInitStep::DisableWifi, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::SetWifiEnabled { enabled: false } =>
            {
                PhyBbInitStep::ConfigureI2cTxRate
            }
            (PhyBbInitStep::ConfigureI2cTxRate, PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::ConfigureI2cTxRate =>
            {
                PhyBbInitStep::ConfigureTxPowerTracking
            }
            (PhyBbInitStep::ConfigureTxPowerTracking, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true }) =>
            {
                PhyBbInitStep::Complete(PhyBbInitOutcome {
                    calibration_performed: self.calibration_performed,
                })
            }
            (PhyBbInitStep::DisableForcedDigitalGain, PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetForcedDigitalGain { enabled: false }) =>
            {
                PhyBbInitStep::TemperatureSecond(
                    crate::analog::temperature::PhyTemperatureTransition::new(),
                )
            }
            (
                PhyBbInitStep::FailureDisableForcedDigitalGain(failure),
                PhyBbInitCompletion::Mmio(completed),
            ) if completed.action == (PhyBbMmioAction::SetForcedDigitalGain { enabled: false }) => {
                PhyBbInitStep::FailureSetIdle(failure)
            }
            (PhyBbInitStep::FailureSetIdle(failure), PhyBbInitCompletion::Mmio(completed))
                if completed.action
                    == (PhyBbMmioAction::SetBasebandMode {
                        mode: PhyBbBasebandMode::Idle,
                    }) =>
            {
                PhyBbInitStep::FailureEnableAgc(failure)
            }
            (PhyBbInitStep::FailureEnableAgc(failure), PhyBbInitCompletion::Mmio(completed))
                if completed.action == PhyBbMmioAction::EnableAgc =>
            {
                PhyBbInitStep::Failed(failure)
            }
            (PhyBbInitStep::Complete(_), _) | (PhyBbInitStep::Failed(_), _) => {
                return Err(PhyBbInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyBbInitTransitionError::WrongCompletion),
        };
        self.step = step;
        Ok(())
    }
}

impl PhyBbInitStep {
    const fn mmio_action(self) -> PhyBbMmioAction {
        match self {
            Self::EnableInitialization => PhyBbMmioAction::EnableBasebandInitialization,
            Self::EnableForcedDigitalGain => {
                PhyBbMmioAction::SetForcedDigitalGain { enabled: true }
            }
            Self::DisableForcedDigitalGain | Self::FailureDisableForcedDigitalGain(_) => {
                PhyBbMmioAction::SetForcedDigitalGain { enabled: false }
            }
            Self::SetCalibrationMode => PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Calibration,
            },
            Self::RxTable(parameters) => PhyBbMmioAction::ConfigureRxTable { parameters },
            Self::RxSaturationPrepare => PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::PrepareCheck,
            },
            Self::RxSaturationFinalize => PhyBbMmioAction::ConfigureRfRxSaturation {
                phase: PhyRfRxSaturationPhase::Finalize,
            },
            Self::RegisterInit(parameters) => PhyBbMmioAction::ConfigurePhyRegisters { parameters },
            Self::UpdateAgc => PhyBbMmioAction::UpdateAgcRegisters,
            Self::UpdatePostInit => PhyBbMmioAction::UpdatePostInitRegisters,
            Self::EnableAgc | Self::FailureEnableAgc(_) => PhyBbMmioAction::EnableAgc,
            Self::SetIdleMode | Self::FailureSetIdle(_) => PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Idle,
            },
            Self::DisableWifi => PhyBbMmioAction::SetWifiEnabled { enabled: false },
            Self::ConfigureI2cTxRate => PhyBbMmioAction::ConfigureI2cTxRate,
            Self::ConfigureTxPowerTracking => {
                PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true }
            }
            _ => panic!("non-MMIO baseband step"),
        }
    }
}

#[cfg(test)]
mod tests;
