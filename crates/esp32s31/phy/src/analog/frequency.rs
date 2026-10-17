//! Rust-owned ESP32-S31 cold frequency-table generation.
//!
//! The pinned reference is `libphy.a[phy_hw_freq.o]::phy_get_rf_freq_init`,
//! size `0x1d8`, and the rev0 ROM leaves `phy_get_data_sat`,
//! `phy_get_freq_mem_param`, `phy_freq_i2c_mem_write`, and
//! `phy_wr_rf_freq_mem`. The vendor parent materializes one three-word record
//! at a time for 85 frequencies beginning at `0x960`.
//!
//! Rust retains only the interpolation scalars and the current entry/word
//! indices. It never allocates or stores the complete 1,020-byte table in
//! SRAM. Every hardware-memory publication is an identity-bound finite MMIO
//! action completed by the caller.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_freq_mem_backup() {}

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_freq_offset_set() {}

use crate::analog::{
    i2c::{PhyI2cAddress, PhyI2cField, analog_registers},
    rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyOutcome, RfpllFrequencyRequest, RfpllFrequencyTransition,
        calculate_rfpll_sdm,
    },
};

use oer_esp32s31_hal::phy::frequency::PhyFrequencyI2cNumberAddresses;

pub const PHY_FREQUENCY_TABLE_ENTRY_COUNT: u8 = 85;
pub const PHY_FREQUENCY_TABLE_FIRST_CODE: u16 = 0x960;
// `phy_get_freq_mem_param(2)` packs three distinct values: the low byte is
// the kind-2 I2C-memory base (0x12), the middle byte is the stride (7), and
// the high byte is the RF-record base (0x20 = 0x12 + 2 * 7).
const PHY_FREQUENCY_I2C_MEMORY_BASE: u16 = 0x12;
const PHY_RF_FREQUENCY_MEMORY_BASE: u16 = 0x20;
const PHY_FREQUENCY_MEMORY_ENTRY_STRIDE: u16 = 7;
const PHY_FREQUENCY_MEMORY_WORD_STRIDE: u16 = 3;
const PHY_FREQUENCY_MEMORY_MODE: u8 = 7;
const CAP_INTERPOLATION_DIVISOR: i32 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableParameters {
    /// Explicit replacement for `phy_param[0x4f]`.
    pub crystal_selector: u8,
    /// Explicit replacement for `phy_param[0x19f]`.
    pub middle_xtal_duty: u8,
    /// Explicit replacement for `phy_param[0x1a0]`.
    pub outer_xtal_duty: u8,
    /// Upper five bits read once from PHY-I2C block `0x63`, register `6`.
    pub sdm_register_six_upper: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyMemoryRecord {
    words: [u32; 3],
}

impl PhyFrequencyMemoryRecord {
    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn words(self) -> [u32; 3] {
        self.words
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableRequest {
    pub parameters: PhyFrequencyTableParameters,
    pub low_frequency_cap: u16,
    pub high_frequency_cap: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableOutcome {
    pub entries_written: u8,
    pub low_frequency_cap: u16,
    pub high_frequency_cap: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyTableAction {
    WriteMemory {
        entry_index: u8,
        word_index: u8,
        address: u16,
        value: u32,
        mode: u8,
    },
    Complete(PhyFrequencyTableOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableCompletion {
    pub entry_index: u8,
    pub word_index: u8,
    pub address: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyTableTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact stateless body of pinned `libphy.a[phy_rx_cal.o]::phy_get_xtal_duty`.
pub const fn phy_frequency_xtal_duty(
    frequency_code: u16,
    middle_xtal_duty: u8,
    outer_xtal_duty: u8,
) -> u8 {
    if frequency_code.wrapping_sub(0x975) <= 38 {
        middle_xtal_duty
    } else {
        outer_xtal_duty
    }
}

const fn interpolated_cap(low: u16, high: u16, index: u8) -> u16 {
    let low = low as i16 as i32;
    let delta = (high as i16 as i32).wrapping_sub(low);
    let interpolated =
        low.wrapping_add(delta.wrapping_mul(index as i32) / CAP_INTERPOLATION_DIVISOR);
    if interpolated < 0 {
        0
    } else if interpolated > 0x1ff {
        0x1ff
    } else {
        interpolated as u16
    }
}

/// Reproduce one packed three-word `phy_wr_rf_freq_mem` input record.
pub const fn phy_frequency_memory_record(
    request: PhyFrequencyTableRequest,
    index: u8,
) -> PhyFrequencyMemoryRecord {
    let frequency_code = PHY_FREQUENCY_TABLE_FIRST_CODE.wrapping_add(index as u16);
    let sdm = calculate_rfpll_sdm(frequency_code, request.parameters.crystal_selector, 0).bytes();
    let cap = interpolated_cap(request.low_frequency_cap, request.high_frequency_cap, index);
    let cap_high = 0xbf | (((cap >> 8) as u8 & 1) << 6);
    let sdm_low = sdm[0] | (request.parameters.sdm_register_six_upper & 0xf8);
    let xtal_duty = phy_frequency_xtal_duty(
        frequency_code,
        request.parameters.middle_xtal_duty,
        request.parameters.outer_xtal_duty,
    );

    PhyFrequencyMemoryRecord {
        words: [
            cap as u8 as u32 | ((cap_high as u32) << 8) | ((sdm_low as u32) << 16),
            sdm[1] as u32 | ((sdm[2] as u32) << 8) | ((sdm[3] as u32) << 16),
            xtal_duty as u32,
        ],
    }
}

/// Caller-driven 85-entry frequency-memory publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyTableTransition {
    request: PhyFrequencyTableRequest,
    entry_index: u8,
    word_index: u8,
}

/// Exact non-I/O arithmetic of complete rev0 ROM
/// `phy_get_freq_mem_addr`: `base + index * stride + offset`, retained to the
/// sixteen-bit frequency-memory address domain.
pub const fn phy_get_freq_mem_addr(base: u32, stride: u32, index: u32, offset: u32) -> u16 {
    base.wrapping_add(index.wrapping_mul(stride))
        .wrapping_add(offset) as u16
}

impl PhyFrequencyTableTransition {
    pub const fn new(request: PhyFrequencyTableRequest) -> Self {
        Self {
            request,
            entry_index: 0,
            word_index: 0,
        }
    }

    const fn address(self) -> u16 {
        phy_get_freq_mem_addr(
            PHY_RF_FREQUENCY_MEMORY_BASE as u32,
            PHY_FREQUENCY_MEMORY_ENTRY_STRIDE as u32,
            self.entry_index as u32,
            self.word_index as u32 * PHY_FREQUENCY_MEMORY_WORD_STRIDE as u32,
        )
    }

    pub const fn action(self) -> PhyFrequencyTableAction {
        if self.entry_index == PHY_FREQUENCY_TABLE_ENTRY_COUNT {
            PhyFrequencyTableAction::Complete(PhyFrequencyTableOutcome {
                entries_written: self.entry_index,
                low_frequency_cap: self.request.low_frequency_cap,
                high_frequency_cap: self.request.high_frequency_cap,
            })
        } else {
            let record = phy_frequency_memory_record(self.request, self.entry_index);
            PhyFrequencyTableAction::WriteMemory {
                entry_index: self.entry_index,
                word_index: self.word_index,
                address: self.address(),
                value: record.words[self.word_index as usize],
                mode: PHY_FREQUENCY_MEMORY_MODE,
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyFrequencyTableCompletion,
    ) -> Result<(), PhyFrequencyTableTransitionError> {
        let PhyFrequencyTableAction::WriteMemory {
            entry_index,
            word_index,
            address,
            ..
        } = self.action()
        else {
            return Err(PhyFrequencyTableTransitionError::AlreadyComplete);
        };
        if completion.entry_index != entry_index
            || completion.word_index != word_index
            || completion.address != address
        {
            return Err(PhyFrequencyTableTransitionError::WrongCompletion);
        }

        if self.word_index == 2 {
            self.word_index = 0;
            self.entry_index += 1;
        } else {
            self.word_index += 1;
        }
        Ok(())
    }
}

const PHY_RF_FREQUENCY_MEMORY_READ_MODE: u8 = 2;
const PHY_RF_FREQUENCY_MEMORY_WRITE_MODE: u8 = 3;

/// Signed capacitor correction accepted by `phy_pll_cap_mem_update`.
///
/// The ROM status-based correction chooses ±2. The current archive's RFPLL
/// search supplies a measured signed delta instead; the memory operation is
/// shared by both callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyCapCorrection(i16);

impl PhyFrequencyCapCorrection {
    pub const fn from_delta(delta: i16) -> Self {
        Self(delta)
    }

    pub const fn delta(self) -> i16 {
        self.0
    }
}

/// Inputs formerly read by `phy_pll_cap_mem_update` from shared globals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyCapMemoryRequest {
    pub correction: PhyFrequencyCapCorrection,
    pub current_channel: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyCapMemoryOutcome {
    pub entries_updated: u8,
    pub correction: PhyFrequencyCapCorrection,
    pub restored_frequency_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyCapMemoryAction {
    ReadMemory {
        entry_index: u8,
        address: u16,
        mode: u8,
    },
    WriteMemory {
        entry_index: u8,
        address: u16,
        value: u32,
        mode: u8,
    },
    RestoreChannelIndex {
        frequency_index: u8,
    },
    Complete(PhyFrequencyCapMemoryOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyCapMemoryCompletion {
    MemoryRead {
        entry_index: u8,
        address: u16,
        mode: u8,
        value: u32,
    },
    MemoryWritten {
        entry_index: u8,
        address: u16,
        mode: u8,
    },
    ChannelIndexRestored {
        frequency_index: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyCapMemoryTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Debug, Eq, PartialEq)]
enum PhyFrequencyCapMemoryStep {
    Read { entry_index: u8 },
    Write { entry_index: u8, value: u32 },
    RestoreChannelIndex,
    Complete,
}

/// Exact arithmetic used by complete rev0 ROM `phy_pll_cap_mem_update` for
/// one RF frequency-memory word.
/// The full signed result is retained: ROM ORs it with the write-mode byte.
/// Truncating it to a payload here changes the operation on underflow.
pub const fn phy_frequency_cap_adjusted_word(
    raw: u32,
    correction: PhyFrequencyCapCorrection,
) -> u32 {
    let cap = ((raw & 0xff) | ((raw >> 6) & 0x100)) as u16;
    let adjusted = cap.wrapping_add(correction.delta() as u16);
    let signed_high = ((adjusted as i16 as i32) >> 8) as u32;
    (raw & 0x0000_bf00) | (adjusted as u8 as u32) | signed_high.wrapping_shl(14)
}

const fn cap_memory_write_mode(value: u32) -> u8 {
    PHY_RF_FREQUENCY_MEMORY_WRITE_MODE | (value >> 24) as u8
}

/// Exact low-byte channel image restored by `phy_pll_cap_mem_update`.
pub const fn phy_frequency_channel_index(channel: u16) -> u8 {
    crate::channel::channel_to_frequency(channel).wrapping_sub(0x60) as u8
}

/// Non-cloneable caller-driven owner of the 85-entry RFPLL cap update.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyFrequencyCapMemoryTransition {
    request: PhyFrequencyCapMemoryRequest,
    step: PhyFrequencyCapMemoryStep,
}

impl PhyFrequencyCapMemoryTransition {
    pub const fn new(request: PhyFrequencyCapMemoryRequest) -> Self {
        Self {
            request,
            step: PhyFrequencyCapMemoryStep::Read { entry_index: 0 },
        }
    }

    const fn address(entry_index: u8) -> u16 {
        phy_get_freq_mem_addr(
            PHY_RF_FREQUENCY_MEMORY_BASE as u32,
            PHY_FREQUENCY_MEMORY_ENTRY_STRIDE as u32,
            entry_index as u32,
            0,
        )
    }

    #[inline(always)]
    pub const fn action(&self) -> PhyFrequencyCapMemoryAction {
        match self.step {
            PhyFrequencyCapMemoryStep::Read { entry_index } => {
                PhyFrequencyCapMemoryAction::ReadMemory {
                    entry_index,
                    address: Self::address(entry_index),
                    mode: PHY_RF_FREQUENCY_MEMORY_READ_MODE,
                }
            }
            PhyFrequencyCapMemoryStep::Write { entry_index, value } => {
                PhyFrequencyCapMemoryAction::WriteMemory {
                    entry_index,
                    address: Self::address(entry_index),
                    value: value & 0x00ff_ffff,
                    mode: cap_memory_write_mode(value),
                }
            }
            PhyFrequencyCapMemoryStep::RestoreChannelIndex => {
                PhyFrequencyCapMemoryAction::RestoreChannelIndex {
                    frequency_index: phy_frequency_channel_index(self.request.current_channel),
                }
            }
            PhyFrequencyCapMemoryStep::Complete => {
                PhyFrequencyCapMemoryAction::Complete(PhyFrequencyCapMemoryOutcome {
                    entries_updated: PHY_FREQUENCY_TABLE_ENTRY_COUNT,
                    correction: self.request.correction,
                    restored_frequency_index: phy_frequency_channel_index(
                        self.request.current_channel,
                    ),
                })
            }
        }
    }

    #[inline(always)]
    pub fn advance(
        &mut self,
        completion: PhyFrequencyCapMemoryCompletion,
    ) -> Result<(), PhyFrequencyCapMemoryTransitionError> {
        self.step = match (&self.step, completion) {
            (
                PhyFrequencyCapMemoryStep::Read { entry_index },
                PhyFrequencyCapMemoryCompletion::MemoryRead {
                    entry_index: completed_entry,
                    address,
                    mode: PHY_RF_FREQUENCY_MEMORY_READ_MODE,
                    value,
                },
            ) if completed_entry == *entry_index && address == Self::address(*entry_index) => {
                PhyFrequencyCapMemoryStep::Write {
                    entry_index: *entry_index,
                    value: phy_frequency_cap_adjusted_word(value, self.request.correction),
                }
            }
            (
                PhyFrequencyCapMemoryStep::Write { entry_index, value },
                PhyFrequencyCapMemoryCompletion::MemoryWritten {
                    entry_index: completed_entry,
                    address,
                    mode,
                },
            ) if completed_entry == *entry_index
                && address == Self::address(*entry_index)
                && mode == cap_memory_write_mode(*value) =>
            {
                let next = entry_index.wrapping_add(1);
                if next == PHY_FREQUENCY_TABLE_ENTRY_COUNT {
                    PhyFrequencyCapMemoryStep::RestoreChannelIndex
                } else {
                    PhyFrequencyCapMemoryStep::Read { entry_index: next }
                }
            }
            (
                PhyFrequencyCapMemoryStep::RestoreChannelIndex,
                PhyFrequencyCapMemoryCompletion::ChannelIndexRestored { frequency_index },
            ) if frequency_index == phy_frequency_channel_index(self.request.current_channel) => {
                PhyFrequencyCapMemoryStep::Complete
            }
            (PhyFrequencyCapMemoryStep::Complete, _) => {
                return Err(PhyFrequencyCapMemoryTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyFrequencyCapMemoryTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyCapMemoryBindingError {
    UnsupportedAction,
}

/// Non-cloneable owner of one RF frequency-memory MMIO transaction.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyFrequencyCapMemoryExternalBinding {
    action: PhyFrequencyCapMemoryAction,
}

impl PhyFrequencyCapMemoryExternalBinding {
    pub const fn lower(
        action: PhyFrequencyCapMemoryAction,
    ) -> Result<Self, PhyFrequencyCapMemoryBindingError> {
        match action {
            PhyFrequencyCapMemoryAction::Complete(_) => {
                Err(PhyFrequencyCapMemoryBindingError::UnsupportedAction)
            }
            _ => Ok(Self { action }),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyFrequencyCapMemoryAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyFrequencyCapMemoryCompletion {
        match self.action {
            PhyFrequencyCapMemoryAction::ReadMemory {
                entry_index,
                address,
                mode,
            } => {
                let value = oer_esp32s31_hal::phy::frequency::read_memory(registers, address, mode);
                PhyFrequencyCapMemoryCompletion::MemoryRead {
                    entry_index,
                    address,
                    mode,
                    value,
                }
            }
            PhyFrequencyCapMemoryAction::WriteMemory {
                entry_index,
                address,
                value,
                mode,
            } => {
                oer_esp32s31_hal::phy::frequency::write_memory(registers, address, value, mode);
                PhyFrequencyCapMemoryCompletion::MemoryWritten {
                    entry_index,
                    address,
                    mode,
                }
            }
            PhyFrequencyCapMemoryAction::RestoreChannelIndex { frequency_index } => {
                oer_esp32s31_hal::phy::frequency::restore_channel_index(registers, frequency_index);
                PhyFrequencyCapMemoryCompletion::ChannelIndexRestored { frequency_index }
            }
            PhyFrequencyCapMemoryAction::Complete(_) => {
                unreachable!("terminal frequency-memory action cannot be externally lowered")
            }
        }
    }
}

/// Publication policy selected by the final argument of complete rev0 ROM
/// `phy_freq_i2c_write_set`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cPublication {
    /// Cold initialization republishes command memory and its number-address
    /// table (`phy_freq_i2c_data_write(1)`).
    MemoryAndAddresses,
    /// Retained wake preserves command memory and republishes only the
    /// number-address table (`phy_freq_i2c_data_write(0)`).
    AddressesOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cRequest {
    /// Explicit replacement for the single bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
    pub publication: PhyFrequencyI2cPublication,
}

impl PhyFrequencyI2cRequest {
    pub const fn cold(front_end_parameter_bit: bool) -> Self {
        Self {
            front_end_parameter_bit,
            publication: PhyFrequencyI2cPublication::MemoryAndAddresses,
        }
    }

    pub const fn retained_wake(front_end_parameter_bit: bool) -> Self {
        Self {
            front_end_parameter_bit,
            publication: PhyFrequencyI2cPublication::AddressesOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cOutcome {
    pub rfpll_register_0b: u8,
    pub sdm_register_0: u8,
    pub front_end_register_3: u8,
    pub number_addresses: PhyFrequencyI2cNumberAddresses,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cAction {
    WriteMasked {
        field: PhyI2cField,
        value: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    WriteMemory {
        descriptor_index: u8,
        copy_index: u8,
        address: u16,
        value: u32,
        mode: u8,
    },
    ConfigureNumberAddresses(PhyFrequencyI2cNumberAddresses),
    Complete(PhyFrequencyI2cOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cCompletion {
    MaskedWrite {
        field: PhyI2cField,
    },
    ByteRead {
        address: PhyI2cAddress,
        value: u8,
    },
    MemoryWrite {
        descriptor_index: u8,
        copy_index: u8,
        address: u16,
    },
    NumberAddressesConfigured(PhyFrequencyI2cNumberAddresses),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFrequencyI2cTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhyFrequencyI2cDescriptor {
    block: u8,
    register: u8,
    encoded_index: u8,
    data: [u8; 3],
}

impl PhyFrequencyI2cDescriptor {
    const fn kind(self) -> u8 {
        self.encoded_index >> 4
    }

    const fn index(self) -> u8 {
        self.encoded_index & 0x0f
    }

    const fn number_address(self) -> u8 {
        match self.kind() {
            0 => self.index(),
            1 => self.index().wrapping_add(1),
            _ => self.index().wrapping_add(4),
        }
    }

    const fn memory_write_count(self) -> u8 {
        if self.kind() == 0 { 3 } else { 1 }
    }

    const fn memory_write(self, copy_index: u8) -> (u16, u32, u8) {
        // `phy_freq_i2c_write_set` stores the register byte before the block
        // byte in frequency command memory: `[block, register, data]` as the
        // little-endian word `data << 16 | register << 8 | block`.
        let register_block = ((self.register as u32) << 8) | self.block as u32;
        let data_word = ((self.data[copy_index as usize] as u32) << 16) | register_block;
        match self.kind() {
            0 => match copy_index {
                0 => (
                    self.index() as u16 * 3,
                    data_word,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
                1 => (
                    (self.index() as u16 + 1) * 3,
                    data_word,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
                _ => (
                    self.index() as u16 * 3 + 6,
                    data_word,
                    PHY_FREQUENCY_MEMORY_MODE,
                ),
            },
            1 => (
                self.index() as u16 * 3 + 9,
                ((self.data[0] as u32) << 16) | register_block,
                PHY_FREQUENCY_MEMORY_MODE,
            ),
            _ => (
                self.index() as u16 * 2 + PHY_FREQUENCY_I2C_MEMORY_BASE,
                register_block,
                3,
            ),
        }
    }
}

const fn frequency_i2c_descriptor(
    index: u8,
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
    front_end_parameter_bit: bool,
) -> PhyFrequencyI2cDescriptor {
    match index {
        0 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 1,
            encoded_index: 0x20,
            data: [0; 3],
        },
        1 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 2,
            encoded_index: 0x21,
            data: [0; 3],
        },
        2 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 0,
            encoded_index: 0x10,
            data: [sdm_register_0 & 0xf7, 0, 0],
        },
        3 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 6,
            encoded_index: 0x22,
            data: [0; 3],
        },
        4 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 5,
            encoded_index: 0x23,
            data: [0; 3],
        },
        5 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 4,
            encoded_index: 0x24,
            data: [0; 3],
        },
        6 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 3,
            encoded_index: 0x25,
            data: [0; 3],
        },
        7 => PhyFrequencyI2cDescriptor {
            block: 0x63,
            register: 0,
            encoded_index: 0x11,
            data: [sdm_register_0 | 8, 0, 0],
        },
        8 => PhyFrequencyI2cDescriptor {
            block: 0x62,
            register: 0x0b,
            encoded_index: 0x12,
            data: [rfpll_register_0b, 0, 0],
        },
        9 => PhyFrequencyI2cDescriptor {
            block: 0x61,
            register: 0x0a,
            encoded_index: 0x26,
            data: [0; 3],
        },
        _ => {
            let selected = front_end_register_3 | if front_end_parameter_bit { 4 } else { 0 };
            PhyFrequencyI2cDescriptor {
                block: 0x67,
                register: 3,
                encoded_index: 0,
                data: [selected, front_end_register_3 & !4, selected],
            }
        }
    }
}

const fn phy_frequency_i2c_number_addresses(
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
    front_end_parameter_bit: bool,
) -> PhyFrequencyI2cNumberAddresses {
    let mut values = [0_u8; 11];
    let mut index = 0;
    while index != values.len() {
        values[index] = frequency_i2c_descriptor(
            index as u8,
            rfpll_register_0b,
            sdm_register_0,
            front_end_register_3,
            front_end_parameter_bit,
        )
        .number_address();
        index += 1;
    }
    match PhyFrequencyI2cNumberAddresses::new(values) {
        Some(addresses) => addresses,
        None => panic!("recovered PHY-I2C number address exceeds its PAC domain"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyFrequencyI2cStep {
    EnableSnapshot,
    ReadRfpll,
    ReadSdm {
        rfpll_register_0b: u8,
    },
    ReadFrontEnd {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
    },
    Memory {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
        front_end_register_3: u8,
        descriptor_index: u8,
        copy_index: u8,
    },
    NumberAddresses {
        rfpll_register_0b: u8,
        sdm_register_0: u8,
        front_end_register_3: u8,
    },
    Complete(PhyFrequencyI2cOutcome),
}

/// Complete caller-driven replacement for both pinned
/// `phy_freq_i2c_data_write(1)` cold publication and
/// `phy_freq_i2c_data_write(0)` retained-wake publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cTransition {
    request: PhyFrequencyI2cRequest,
    step: PhyFrequencyI2cStep,
}

impl PhyFrequencyI2cTransition {
    pub const fn new(request: PhyFrequencyI2cRequest) -> Self {
        Self {
            request,
            step: PhyFrequencyI2cStep::EnableSnapshot,
        }
    }

    pub const fn action(self) -> PhyFrequencyI2cAction {
        match self.step {
            PhyFrequencyI2cStep::EnableSnapshot => PhyFrequencyI2cAction::WriteMasked {
                field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
                value: 1,
            },
            PhyFrequencyI2cStep::ReadRfpll => PhyFrequencyI2cAction::ReadByte {
                address: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE.address(),
            },
            PhyFrequencyI2cStep::ReadSdm { .. } => PhyFrequencyI2cAction::ReadByte {
                address: analog_registers::RFPLL_SDM_UPDATE_ENABLE.address(),
            },
            PhyFrequencyI2cStep::ReadFrontEnd { .. } => PhyFrequencyI2cAction::ReadByte {
                address: analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE.address(),
            },
            PhyFrequencyI2cStep::Memory {
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
                descriptor_index,
                copy_index,
            } => {
                let descriptor = frequency_i2c_descriptor(
                    descriptor_index,
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                let (address, value, mode) = descriptor.memory_write(copy_index);
                PhyFrequencyI2cAction::WriteMemory {
                    descriptor_index,
                    copy_index,
                    address,
                    value,
                    mode,
                }
            }
            PhyFrequencyI2cStep::NumberAddresses {
                rfpll_register_0b,
                sdm_register_0,
                front_end_register_3,
            } => {
                PhyFrequencyI2cAction::ConfigureNumberAddresses(phy_frequency_i2c_number_addresses(
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                ))
            }
            PhyFrequencyI2cStep::Complete(outcome) => PhyFrequencyI2cAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyFrequencyI2cCompletion,
    ) -> Result<(), PhyFrequencyI2cTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyFrequencyI2cStep::EnableSnapshot,
                PhyFrequencyI2cCompletion::MaskedWrite {
                    field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
                },
            ) => PhyFrequencyI2cStep::ReadRfpll,
            (
                PhyFrequencyI2cStep::ReadRfpll,
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE.address() => {
                PhyFrequencyI2cStep::ReadSdm {
                    rfpll_register_0b: value,
                }
            }
            (
                PhyFrequencyI2cStep::ReadSdm { rfpll_register_0b },
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == analog_registers::RFPLL_SDM_UPDATE_ENABLE.address() => {
                PhyFrequencyI2cStep::ReadFrontEnd {
                    rfpll_register_0b,
                    sdm_register_0: value,
                }
            }
            (
                PhyFrequencyI2cStep::ReadFrontEnd {
                    rfpll_register_0b,
                    sdm_register_0,
                },
                PhyFrequencyI2cCompletion::ByteRead { address, value },
            ) if address == analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE.address() => {
                match self.request.publication {
                    PhyFrequencyI2cPublication::MemoryAndAddresses => PhyFrequencyI2cStep::Memory {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3: value,
                        descriptor_index: 0,
                        copy_index: 0,
                    },
                    PhyFrequencyI2cPublication::AddressesOnly => {
                        PhyFrequencyI2cStep::NumberAddresses {
                            rfpll_register_0b,
                            sdm_register_0,
                            front_end_register_3: value,
                        }
                    }
                }
            }
            (
                PhyFrequencyI2cStep::Memory {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    descriptor_index,
                    copy_index,
                },
                PhyFrequencyI2cCompletion::MemoryWrite {
                    descriptor_index: completed_descriptor,
                    copy_index: completed_copy,
                    address: completed_address,
                },
            ) => {
                let descriptor = frequency_i2c_descriptor(
                    descriptor_index,
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                let (address, _, _) = descriptor.memory_write(copy_index);
                if descriptor_index != completed_descriptor
                    || copy_index != completed_copy
                    || address != completed_address
                {
                    return Err(PhyFrequencyI2cTransitionError::WrongCompletion);
                }
                if copy_index + 1 != descriptor.memory_write_count() {
                    PhyFrequencyI2cStep::Memory {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                        descriptor_index,
                        copy_index: copy_index + 1,
                    }
                } else if descriptor_index == 10 {
                    PhyFrequencyI2cStep::NumberAddresses {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                    }
                } else {
                    PhyFrequencyI2cStep::Memory {
                        rfpll_register_0b,
                        sdm_register_0,
                        front_end_register_3,
                        descriptor_index: descriptor_index + 1,
                        copy_index: 0,
                    }
                }
            }
            (
                PhyFrequencyI2cStep::NumberAddresses {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                },
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(completed),
            ) => {
                let expected = phy_frequency_i2c_number_addresses(
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    self.request.front_end_parameter_bit,
                );
                if completed != expected {
                    return Err(PhyFrequencyI2cTransitionError::WrongCompletion);
                }
                PhyFrequencyI2cStep::Complete(PhyFrequencyI2cOutcome {
                    rfpll_register_0b,
                    sdm_register_0,
                    front_end_register_3,
                    number_addresses: expected,
                })
            }
            (PhyFrequencyI2cStep::Complete(_), _) => {
                return Err(PhyFrequencyI2cTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyFrequencyI2cTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitControl {
    /// Explicit replacement for the hidden `phy_param[0x193]` branch.
    pub frequency_register_parameter_override: bool,
    /// Rust-owned replacement for `phy_param[0xa4] & 0x20`.
    pub frequency_table_initialized: bool,
    /// Explicit replacement for the bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitRequest {
    /// Explicit replacement for the hidden `phy_param[0x193]` branch.
    pub frequency_register_parameter_override: bool,
    /// Rust-owned replacement for `phy_param[0xa4] & 0x20`.
    pub frequency_table_initialized: bool,
    /// Explicit replacement for `phy_param[0x4f]`.
    pub crystal_selector: u8,
    /// Explicit replacement for `phy_param[0x19f]`.
    pub middle_xtal_duty: u8,
    /// Explicit replacement for `phy_param[0x1a0]`.
    pub outer_xtal_duty: u8,
    /// Explicit replacement for the bit read from `phy_param[0x1af]`.
    pub front_end_parameter_bit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyCalibrationOutcome {
    pub nominal: RfpllFrequencyOutcome,
    pub low: RfpllFrequencyOutcome,
    pub high: RfpllFrequencyOutcome,
    pub table: PhyFrequencyTableOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitOutcome {
    pub table_was_initialized: bool,
    pub table_is_initialized: bool,
    pub calibration: Option<PhyChannelFrequencyCalibrationOutcome>,
    pub i2c: PhyFrequencyI2cOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyRfpllPoint {
    Nominal,
    Low,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitFailure {
    Rfpll {
        point: PhyChannelFrequencyRfpllPoint,
        failure: RfpllFrequencyFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitAction {
    ConfigureFrequencyRegisters {
        parameter_override: bool,
    },
    WriteMasked {
        field: PhyI2cField,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    Rfpll {
        point: PhyChannelFrequencyRfpllPoint,
        action: RfpllFrequencyAction,
    },
    Table(PhyFrequencyTableAction),
    I2c(PhyFrequencyI2cAction),
    Complete(PhyChannelFrequencyInitOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitCompletion {
    FrequencyRegistersConfigured { parameter_override: bool },
    MaskedWrite { field: PhyI2cField },
    ByteWrite { address: PhyI2cAddress },
    ByteRead { address: PhyI2cAddress, value: u8 },
    Rfpll(RfpllFrequencyCompletion),
    Table(PhyFrequencyTableCompletion),
    I2c(PhyFrequencyI2cCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChannelFrequencyInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyChannelFrequencyInitStep {
    ConfigureRegisters,
    InitialCapLow,
    InitialCapHigh,
    DisableRfpll,
    Nominal(RfpllFrequencyTransition),
    EnableCapRange {
        nominal: RfpllFrequencyOutcome,
    },
    EnableRfpll {
        nominal: RfpllFrequencyOutcome,
    },
    Low {
        nominal: RfpllFrequencyOutcome,
        transition: RfpllFrequencyTransition,
    },
    High {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        transition: RfpllFrequencyTransition,
    },
    ReadSdmUpper {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        high: RfpllFrequencyOutcome,
    },
    Table {
        nominal: RfpllFrequencyOutcome,
        low: RfpllFrequencyOutcome,
        high: RfpllFrequencyOutcome,
        transition: PhyFrequencyTableTransition,
    },
    I2c {
        calibration: Option<PhyChannelFrequencyCalibrationOutcome>,
        transition: PhyFrequencyI2cTransition,
    },
    Complete(PhyChannelFrequencyInitOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyChannelFrequencyRfpllAdvance {
    Pending,
    Complete(RfpllFrequencyOutcome),
    Failed(PhyChannelFrequencyInitFailure),
}

/// Caller-driven replacement for the complete pinned
/// `phy_set_chan_freq_hw_init(2, 4)` graph.
///
/// The transition owns the former `phy_param[0xa4]` initialized bit. Every
/// PHY-I2C transaction, RFPLL timer edge, frequency-memory publication, and
/// final number-address register image is externally completed. There is no
/// allocation, internal polling, delay loop, callback, or mutable C state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannelFrequencyInitTransition {
    request: PhyChannelFrequencyInitRequest,
    step: PhyChannelFrequencyInitStep,
}

impl PhyChannelFrequencyInitTransition {
    const SDM_REGISTER_SIX: PhyI2cAddress = analog_registers::RFPLL_SDM_LOW.address();

    pub const fn new(request: PhyChannelFrequencyInitRequest) -> Self {
        Self {
            request,
            step: PhyChannelFrequencyInitStep::ConfigureRegisters,
        }
    }

    const fn rfpll_transition(self, frequency_code: u16) -> RfpllFrequencyTransition {
        RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: self.request.crystal_selector,
            frequency_code,
            offset: 0,
        })
    }

    pub const fn action(self) -> PhyChannelFrequencyInitAction {
        match self.step {
            PhyChannelFrequencyInitStep::ConfigureRegisters => {
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                    parameter_override: self.request.frequency_register_parameter_override,
                }
            }
            PhyChannelFrequencyInitStep::InitialCapLow => {
                PhyChannelFrequencyInitAction::WriteByte {
                    address: analog_registers::RFPLL_CAPACITOR_LOW,
                    value: 0xc8,
                }
            }
            PhyChannelFrequencyInitStep::InitialCapHigh => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    field: analog_registers::RFPLL_CAPACITOR_HIGH,
                    value: 0,
                }
            }
            PhyChannelFrequencyInitStep::DisableRfpll => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_HIGH,
                    value: 0,
                }
            }
            PhyChannelFrequencyInitStep::Nominal(transition) => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::Nominal,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::EnableCapRange { .. } => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_LOW,
                    value: 0x3f,
                }
            }
            PhyChannelFrequencyInitStep::EnableRfpll { .. } => {
                PhyChannelFrequencyInitAction::WriteMasked {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_HIGH,
                    value: 1,
                }
            }
            PhyChannelFrequencyInitStep::Low { transition, .. } => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::Low,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::High { transition, .. } => {
                PhyChannelFrequencyInitAction::Rfpll {
                    point: PhyChannelFrequencyRfpllPoint::High,
                    action: transition.action(),
                }
            }
            PhyChannelFrequencyInitStep::ReadSdmUpper { .. } => {
                PhyChannelFrequencyInitAction::ReadByte {
                    address: Self::SDM_REGISTER_SIX,
                }
            }
            PhyChannelFrequencyInitStep::Table { transition, .. } => {
                PhyChannelFrequencyInitAction::Table(transition.action())
            }
            PhyChannelFrequencyInitStep::I2c { transition, .. } => {
                PhyChannelFrequencyInitAction::I2c(transition.action())
            }
            PhyChannelFrequencyInitStep::Complete(outcome) => {
                PhyChannelFrequencyInitAction::Complete(outcome)
            }
            PhyChannelFrequencyInitStep::Failed(failure) => {
                PhyChannelFrequencyInitAction::Failed(failure)
            }
        }
    }

    fn advance_rfpll(
        point: PhyChannelFrequencyRfpllPoint,
        transition: &mut RfpllFrequencyTransition,
        completion: RfpllFrequencyCompletion,
    ) -> Result<PhyChannelFrequencyRfpllAdvance, PhyChannelFrequencyInitTransitionError> {
        transition
            .advance(completion)
            .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
        match transition.action() {
            RfpllFrequencyAction::Complete(outcome) => {
                Ok(PhyChannelFrequencyRfpllAdvance::Complete(outcome))
            }
            RfpllFrequencyAction::Failed(failure) => Ok(PhyChannelFrequencyRfpllAdvance::Failed(
                PhyChannelFrequencyInitFailure::Rfpll { point, failure },
            )),
            _ => Ok(PhyChannelFrequencyRfpllAdvance::Pending),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyChannelFrequencyInitCompletion,
    ) -> Result<(), PhyChannelFrequencyInitTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyChannelFrequencyInitStep::ConfigureRegisters,
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override,
                },
            ) if parameter_override == self.request.frequency_register_parameter_override => {
                if self.request.frequency_table_initialized {
                    PhyChannelFrequencyInitStep::I2c {
                        calibration: None,
                        transition: PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::cold(
                            self.request.front_end_parameter_bit,
                        )),
                    }
                } else {
                    PhyChannelFrequencyInitStep::InitialCapLow
                }
            }
            (
                PhyChannelFrequencyInitStep::InitialCapLow,
                PhyChannelFrequencyInitCompletion::ByteWrite { address },
            ) if address == analog_registers::RFPLL_CAPACITOR_LOW => {
                PhyChannelFrequencyInitStep::InitialCapHigh
            }
            (
                PhyChannelFrequencyInitStep::InitialCapHigh,
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    field: analog_registers::RFPLL_CAPACITOR_HIGH,
                },
            ) => PhyChannelFrequencyInitStep::DisableRfpll,
            (
                PhyChannelFrequencyInitStep::DisableRfpll,
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_HIGH,
                },
            ) => PhyChannelFrequencyInitStep::Nominal(self.rfpll_transition(0x985)),
            (
                PhyChannelFrequencyInitStep::Nominal(mut transition),
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::Nominal,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(nominal) => {
                        PhyChannelFrequencyInitStep::EnableCapRange { nominal }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => {
                        PhyChannelFrequencyInitStep::Nominal(transition)
                    }
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::EnableCapRange { nominal },
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_LOW,
                },
            ) => PhyChannelFrequencyInitStep::EnableRfpll { nominal },
            (
                PhyChannelFrequencyInitStep::EnableRfpll { nominal },
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    field: analog_registers::RFPLL_INITIAL_CONFIGURATION_HIGH,
                },
            ) => PhyChannelFrequencyInitStep::Low {
                nominal,
                transition: self.rfpll_transition(0x960),
            },
            (
                PhyChannelFrequencyInitStep::Low {
                    nominal,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::Low,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(low) => {
                        PhyChannelFrequencyInitStep::High {
                            nominal,
                            low,
                            transition: self.rfpll_transition(0x9a0),
                        }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => PhyChannelFrequencyInitStep::Low {
                        nominal,
                        transition,
                    },
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::High {
                    nominal,
                    low,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Rfpll(completion),
            ) => {
                match Self::advance_rfpll(
                    PhyChannelFrequencyRfpllPoint::High,
                    &mut transition,
                    completion,
                )? {
                    PhyChannelFrequencyRfpllAdvance::Complete(high) => {
                        PhyChannelFrequencyInitStep::ReadSdmUpper { nominal, low, high }
                    }
                    PhyChannelFrequencyRfpllAdvance::Pending => PhyChannelFrequencyInitStep::High {
                        nominal,
                        low,
                        transition,
                    },
                    PhyChannelFrequencyRfpllAdvance::Failed(failure) => {
                        PhyChannelFrequencyInitStep::Failed(failure)
                    }
                }
            }
            (
                PhyChannelFrequencyInitStep::ReadSdmUpper { nominal, low, high },
                PhyChannelFrequencyInitCompletion::ByteRead { address, value },
            ) if address == Self::SDM_REGISTER_SIX => PhyChannelFrequencyInitStep::Table {
                nominal,
                low,
                high,
                transition: PhyFrequencyTableTransition::new(PhyFrequencyTableRequest {
                    parameters: PhyFrequencyTableParameters {
                        crystal_selector: self.request.crystal_selector,
                        middle_xtal_duty: self.request.middle_xtal_duty,
                        outer_xtal_duty: self.request.outer_xtal_duty,
                        sdm_register_six_upper: value & 0xf8,
                    },
                    // The pinned archive redundantly reads each cap immediately
                    // after the corresponding complete RFPLL call. The child
                    // transition already owns those exact final values.
                    low_frequency_cap: low.final_cap,
                    high_frequency_cap: high.final_cap,
                }),
            },
            (
                PhyChannelFrequencyInitStep::Table {
                    nominal,
                    low,
                    high,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::Table(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyFrequencyTableAction::Complete(table) => PhyChannelFrequencyInitStep::I2c {
                        calibration: Some(PhyChannelFrequencyCalibrationOutcome {
                            nominal,
                            low,
                            high,
                            table,
                        }),
                        transition: PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::cold(
                            self.request.front_end_parameter_bit,
                        )),
                    },
                    _ => PhyChannelFrequencyInitStep::Table {
                        nominal,
                        low,
                        high,
                        transition,
                    },
                }
            }
            (
                PhyChannelFrequencyInitStep::I2c {
                    calibration,
                    mut transition,
                },
                PhyChannelFrequencyInitCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChannelFrequencyInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyFrequencyI2cAction::Complete(i2c) => {
                        PhyChannelFrequencyInitStep::Complete(PhyChannelFrequencyInitOutcome {
                            table_was_initialized: self.request.frequency_table_initialized,
                            table_is_initialized: true,
                            calibration,
                            i2c,
                        })
                    }
                    _ => PhyChannelFrequencyInitStep::I2c {
                        calibration,
                        transition,
                    },
                }
            }
            (PhyChannelFrequencyInitStep::Complete(_), _)
            | (PhyChannelFrequencyInitStep::Failed(_), _) => {
                return Err(PhyChannelFrequencyInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyChannelFrequencyInitTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests;
