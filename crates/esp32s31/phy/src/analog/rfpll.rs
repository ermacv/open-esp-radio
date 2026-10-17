//! Event-driven ESP32-S31 RFPLL frequency programming.
//!
//! The reference is the pinned archive graph rooted at
//! `phy_set_rf_freq_offset_new`. Its `phy_set_rfpll_freq_new` repeats rev0 ROM
//! `phy_set_rfpll_freq` through `phy_rfpll_set_freq`, `phy_write_rfpll_sdm`,
//! `phy_restart_cal`, `phy_wait_rfpll_cal_end` and `phy_read_pll_cap`, then
//! tail-calls the archive capacitor search `phy_rfpll_cap_init_cal_new`
//! instead of ROM `phy_rfpll_cap_init_cal`. The table-selected channel path is
//! `phy_set_channel_rfpll_freq_new`; its `phy_freq_set_reg` is a ROM jump to
//! `phy_nrx_freq_set`.
//!
//! The vendor busy-waits through synchronous PHY-I2C calls, delays inside two
//! loops and prints after a missed lock deadline. Rust exposes every I2C
//! transaction and timer interval as an identity-bound external completion.
//! A missed lock remains ordinary outcome data.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the ESP32-S31 body is one
/// `ret` and does not touch shared RFPLL state.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_bbpll_en_usb() {}

/// Return the exact RF-calibration data version of the pinned archive's
/// `phy_get_rf_cal_version`.
#[inline]
pub const fn phy_get_rf_cal_version() -> u32 {
    102
}

use crate::analog::i2c::{PhyI2cAddress, PhyI2cField, analog_registers};

const LOCK_ATTEMPTS: u8 = 100;
/// Capacitor-status samples one search direction permits.
pub const CAP_SEARCH_SAMPLES_PER_DIRECTION: u8 = 30;
/// Direction-specific boundary statuses which end one search direction early.
const CAP_SEARCH_BOUNDARIES: u8 = 2;
/// Two-bit capacitor-search statuses.
const CAP_STATUS_ACCEPTED: u8 = 0;
const CAP_STATUS_INCREASE: u8 = 1;
const CAP_STATUS_DECREASE: u8 = 2;

/// A programmable RFPLL capacitor code: nine bits, the low byte at
/// `RFPLL_CAPACITOR_LOW` and bit 8 in `RFPLL_CAPACITOR_HIGH`.
///
/// SOURCE(esp32s31): rev0 ROM `phy_write_pll_cap` clamps a negative request to
/// zero, writes the low byte, then writes `cap >> 8` through
/// `phy_i2c_writeReg_Mask(0x62, 0x02, 6, 6)` without masking the value, so a
/// code above `0x1ff` leaks into bit 7 of that byte,
/// `RFPLL_INITIAL_CONFIGURATION_HIGH`. The pinned archive search
/// `phy_rfpll_cap_init_cal_new` does not bound its upward candidates, so an
/// initial capacitor of `0x1e2` or more reaches that leak. The archive also
/// averages the requested sixteen-bit candidates, including downward ones
/// wrapped below zero that ROM programs as zero, so an initial capacitor
/// below 30 can average far above `0x1ff`. Production never writes the
/// neighboring field: the code is bounded here, an upward search ends at the
/// bound as if its samples were exhausted, and both searches average the
/// codes actually programmed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllCapacitorCode(u16);

impl RfpllCapacitorCode {
    /// The largest nine-bit code.
    pub const MAX: u16 = 0x1ff;

    /// The code `value`, or `None` above [`Self::MAX`].
    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The code a signed search request programs: ROM clamps a negative
    /// request to zero; a request above [`Self::MAX`] is not programmable.
    pub const fn from_request(requested: i16) -> Option<Self> {
        if requested < 0 {
            Some(Self(0))
        } else {
            Self::new(requested as u16)
        }
    }

    /// The code a search candidate programs; a downward candidate wrapped
    /// below zero programs zero. The searches keep every candidate within
    /// [`Self::MAX`], and the average of codes is a code.
    pub const fn programmed(candidate: u16) -> Self {
        match Self::from_request(candidate as i16) {
            Some(code) => code,
            None => Self(Self::MAX),
        }
    }

    /// The code as a number.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// The low byte, for `RFPLL_CAPACITOR_LOW`.
    pub const fn low_byte(self) -> u8 {
        self.0 as u8
    }

    /// Bit 8, for the one-bit `RFPLL_CAPACITOR_HIGH`.
    pub const fn high_bit(self) -> u8 {
        (self.0 >> 8) as u8
    }
}

/// SDM bytes `phy_write_rfpll_sdm` programs, least significant first. The ROM
/// also stores bit 27 into a fifth byte of its caller's scratch buffer, which
/// neither `phy_write_rfpll_sdm` nor any caller reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllSdmImage {
    bytes: [u8; 4],
}

impl RfpllSdmImage {
    pub const fn bytes(self) -> [u8; 4] {
        self.bytes
    }
}

/// Stateless body of rev0 ROM `phy_rfpll_set_freq`, less its unread fifth byte.
pub const fn calculate_rfpll_sdm(
    frequency_code: u16,
    crystal_selector: u8,
    offset: u8,
) -> RfpllSdmImage {
    let selector_index = crystal_selector.wrapping_sub(1);
    let divisor = match selector_index {
        0 => 0x1a_u64,
        1 => 0x20_u64,
        2 => 0x30_u64,
        _ => 0x28_u64,
    };

    let scaled_offset = ((offset as u32).wrapping_shl(18) as i32 / 1_000) as u32;
    let scaled_frequency = (frequency_code as u32).wrapping_shl(18);
    let scaled = scaled_offset.wrapping_add(scaled_frequency);
    let (shift, first_divisor) = if frequency_code > 4_000 {
        (5_u32, 27_u64)
    } else {
        (3_u32, 3_u64)
    };
    let encoded =
        ((((scaled as u64) << shift) / first_divisor / divisor) as u32).wrapping_add(0xff00_0000);

    RfpllSdmImage {
        bytes: [
            (encoded & 0x7) as u8,
            (encoded >> 3) as u8,
            (encoded >> 11) as u8,
            (encoded >> 19) as u8,
        ],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyRequest {
    pub crystal_selector: u8,
    pub frequency_code: u16,
    pub offset: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyOutcome {
    pub lock_observed: bool,
    pub initial_cap: u16,
    pub final_cap: u16,
    pub accepted_cap_samples: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyFailure {
    /// The table-selection entry supports only the initialized 2.4-GHz domain.
    UnsupportedChannelFrequency(u16),
    /// Defensive terminal retained for callers which persist this public
    /// outcome. The bounded archive search completes both directions,
    /// including the no-accepted-sample case, so the exact transition does
    /// not normally emit it.
    CapacitorSearchDeadlineExceeded {
        initial_cap: u16,
        accepted_samples: u8,
        offset: u8,
    },
    FrequencyReadyDeadlineExceeded {
        samples: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyAction {
    StartChannelSwitch {
        frequency_index: u8,
        crystal_selector: u8,
    },
    ClearChannelSwitch,
    ReadChannelReady {
        samples: u32,
    },
    ConfigureNrx {
        frequency_mhz: u16,
    },
    WriteMasked {
        field: PhyI2cField,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    ReadMasked {
        field: PhyI2cField,
    },
    ReadByte {
        address: PhyI2cAddress,
    },
    DelayMicros(u32),
    Complete(RfpllFrequencyOutcome),
    Failed(RfpllFrequencyFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyCompletion {
    ChannelSwitchStarted {
        frequency_index: u8,
        crystal_selector: u8,
    },
    ChannelSwitchCleared,
    ChannelReadyObserved {
        ready: bool,
    },
    ChannelReadyTimedOut,
    NrxConfigured {
        frequency_mhz: u16,
    },
    MaskedWrite {
        field: PhyI2cField,
    },
    ByteWrite {
        address: PhyI2cAddress,
    },
    MaskedRead {
        field: PhyI2cField,
        value: u8,
    },
    ByteRead {
        address: PhyI2cAddress,
        value: u8,
    },
    DelayElapsed(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapSearchPhase {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CapSearchState {
    initial: u16,
    phase: CapSearchPhase,
    phase_attempts: u8,
    boundaries: u8,
    accepted: u8,
    sum: u16,
    lock_observed: bool,
}

impl CapSearchState {
    const fn candidate(self) -> u16 {
        match self.phase {
            CapSearchPhase::Down => self.initial.wrapping_sub(self.phase_attempts as u16),
            CapSearchPhase::Up => self
                .initial
                .wrapping_add(1)
                .wrapping_add(self.phase_attempts as u16),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapWriteContinuation {
    Search(CapSearchState),
    Final {
        initial: u16,
        final_cap: u16,
        accepted: u8,
        lock_observed: bool,
    },
}

impl CapWriteContinuation {
    const fn value(self) -> u16 {
        match self {
            Self::Search(search) => search.candidate(),
            Self::Final { final_cap, .. } => final_cap,
        }
    }

    const fn programmed_value(self) -> RfpllCapacitorCode {
        RfpllCapacitorCode::programmed(self.value())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfpllFrequencyStep {
    ChannelStart,
    ChannelStartDelay,
    ChannelClear,
    ChannelSettleDelay,
    ChannelReady {
        samples: u32,
    },
    ChannelNrx,
    /// Direct programming carries the SDM image it writes after the
    /// initial writes; table-selected channels never compute one.
    InitialWrite {
        index: u8,
        sdm: RfpllSdmImage,
    },
    SdmWrite {
        index: u8,
        sdm: RfpllSdmImage,
    },
    RestartWrite(u8),
    LockDelay {
        attempt: u8,
    },
    LockRead {
        attempt: u8,
    },
    CapLowRead {
        lock_observed: bool,
    },
    CapHighRead {
        low: u8,
        lock_observed: bool,
    },
    EnableCapSearch {
        initial: u16,
        lock_observed: bool,
    },
    CapWriteLow(CapWriteContinuation),
    CapWriteHigh(CapWriteContinuation),
    CapDelay(CapWriteContinuation),
    CapStatusRead(CapSearchState),
    Complete(RfpllFrequencyOutcome),
    Failed(RfpllFrequencyFailure),
}

/// Heap-free, caller-driven replacement for the complete RFPLL frequency
/// programming graph used by crystal-duty calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllFrequencyTransition {
    request: RfpllFrequencyRequest,
    step: RfpllFrequencyStep,
}

impl RfpllFrequencyTransition {
    const fn programmed_frequency(request: RfpllFrequencyRequest) -> u16 {
        crate::channel::channel_to_frequency(request.frequency_code)
    }

    /// Program the synthesizer directly, including capacitor calibration.
    /// This is the `phy_set_rf_freq_offset_new` path used to build channel tables;
    /// `frequency_code` is MHz and must not select an already-built table.
    pub const fn new(request: RfpllFrequencyRequest) -> Self {
        Self {
            request,
            step: RfpllFrequencyStep::InitialWrite {
                index: 0,
                sdm: calculate_rfpll_sdm(
                    request.frequency_code,
                    request.crystal_selector,
                    request.offset,
                ),
            },
        }
    }

    /// Select an initialized 2.4-GHz frequency table, then update NRX.
    /// Matches the supported `phy_set_channel_rfpll_freq_new` branch for both a
    /// channel number and MHz input. It does not establish analog PLL lock.
    pub const fn channel(request: RfpllFrequencyRequest) -> Self {
        let frequency = Self::programmed_frequency(request);
        Self {
            request,
            step: if frequency.wrapping_sub(2_400) <= 84 {
                RfpllFrequencyStep::ChannelStart
            } else {
                RfpllFrequencyStep::Failed(RfpllFrequencyFailure::UnsupportedChannelFrequency(
                    frequency,
                ))
            },
        }
    }

    const fn initial_write(index: u8) -> RfpllFrequencyAction {
        let (field, value) = match index {
            0 => (analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE, 0),
            1 => (analog_registers::RFPLL_INITIAL_CONFIGURATION_HIGH, 1),
            _ => (analog_registers::RFPLL_INITIAL_CONFIGURATION_LOW, 0x3f),
        };
        RfpllFrequencyAction::WriteMasked { field, value }
    }

    const fn sdm_write(index: u8, sdm: RfpllSdmImage) -> RfpllFrequencyAction {
        let bytes = sdm.bytes;
        match index {
            0 => RfpllFrequencyAction::WriteMasked {
                field: analog_registers::RFPLL_SDM_UPDATE_ENABLE,
                value: 0,
            },
            1 => RfpllFrequencyAction::WriteByte {
                address: analog_registers::RFPLL_SDM_MOST_SIGNIFICANT_BYTE,
                value: bytes[3],
            },
            2 => RfpllFrequencyAction::WriteByte {
                address: analog_registers::RFPLL_SDM_UPPER_MIDDLE_BYTE,
                value: bytes[2],
            },
            3 => RfpllFrequencyAction::WriteByte {
                address: analog_registers::RFPLL_SDM_LOWER_MIDDLE_BYTE,
                value: bytes[1],
            },
            4 => RfpllFrequencyAction::WriteMasked {
                field: analog_registers::RFPLL_SDM_LOW,
                value: bytes[0],
            },
            _ => RfpllFrequencyAction::WriteMasked {
                field: analog_registers::RFPLL_SDM_UPDATE_ENABLE,
                value: 1,
            },
        }
    }

    const fn restart_write(index: u8) -> RfpllFrequencyAction {
        let (field, value) = match index {
            0 => (analog_registers::RFPLL_RESTART_HIGH, 0),
            1 => (analog_registers::RFPLL_RESTART_LOW, 0),
            2 => (analog_registers::RFPLL_RESTART_LOW, 1),
            _ => (analog_registers::RFPLL_RESTART_HIGH, 1),
        };
        RfpllFrequencyAction::WriteMasked { field, value }
    }

    pub const fn action(self) -> RfpllFrequencyAction {
        match self.step {
            RfpllFrequencyStep::ChannelStart => RfpllFrequencyAction::StartChannelSwitch {
                frequency_index: Self::programmed_frequency(self.request).wrapping_sub(2_400) as u8,
                crystal_selector: self.request.crystal_selector,
            },
            RfpllFrequencyStep::ChannelStartDelay => RfpllFrequencyAction::DelayMicros(1),
            RfpllFrequencyStep::ChannelClear => RfpllFrequencyAction::ClearChannelSwitch,
            RfpllFrequencyStep::ChannelSettleDelay => RfpllFrequencyAction::DelayMicros(10),
            RfpllFrequencyStep::ChannelReady { samples } => {
                RfpllFrequencyAction::ReadChannelReady { samples }
            }
            RfpllFrequencyStep::ChannelNrx => RfpllFrequencyAction::ConfigureNrx {
                frequency_mhz: Self::programmed_frequency(self.request),
            },
            RfpllFrequencyStep::InitialWrite { index, .. } => Self::initial_write(index),
            RfpllFrequencyStep::SdmWrite { index, sdm } => Self::sdm_write(index, sdm),
            RfpllFrequencyStep::RestartWrite(index) => Self::restart_write(index),
            RfpllFrequencyStep::LockDelay { .. } => RfpllFrequencyAction::DelayMicros(20),
            RfpllFrequencyStep::LockRead { .. } => RfpllFrequencyAction::ReadMasked {
                field: analog_registers::RFPLL_LOCK_STATUS,
            },
            RfpllFrequencyStep::CapLowRead { .. } => RfpllFrequencyAction::ReadByte {
                address: analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW,
            },
            RfpllFrequencyStep::CapHighRead { .. } => RfpllFrequencyAction::ReadMasked {
                field: analog_registers::RFPLL_CALIBRATED_CAPACITOR_HIGH,
            },
            RfpllFrequencyStep::EnableCapSearch { .. } => RfpllFrequencyAction::WriteMasked {
                field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
                value: 1,
            },
            RfpllFrequencyStep::CapWriteLow(continuation) => RfpllFrequencyAction::WriteByte {
                address: analog_registers::RFPLL_CAPACITOR_LOW,
                value: continuation.programmed_value().low_byte(),
            },
            RfpllFrequencyStep::CapWriteHigh(continuation) => RfpllFrequencyAction::WriteMasked {
                field: analog_registers::RFPLL_CAPACITOR_HIGH,
                value: continuation.programmed_value().high_bit(),
            },
            RfpllFrequencyStep::CapDelay(_) => RfpllFrequencyAction::DelayMicros(5),
            RfpllFrequencyStep::CapStatusRead(_) => RfpllFrequencyAction::ReadMasked {
                field: analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS,
            },
            RfpllFrequencyStep::Complete(outcome) => RfpllFrequencyAction::Complete(outcome),
            RfpllFrequencyStep::Failed(failure) => RfpllFrequencyAction::Failed(failure),
        }
    }

    fn matches_write(action: RfpllFrequencyAction, completion: RfpllFrequencyCompletion) -> bool {
        match (action, completion) {
            (
                RfpllFrequencyAction::WriteMasked { field, .. },
                RfpllFrequencyCompletion::MaskedWrite { field: completed },
            ) => field == completed,
            (
                RfpllFrequencyAction::WriteByte { address, .. },
                RfpllFrequencyCompletion::ByteWrite { address: completed },
            ) => address == completed,
            _ => false,
        }
    }

    /// Program the next candidate, or end an upward phase at the nine-bit
    /// bound as if its samples were exhausted.
    fn continue_cap_search(&mut self, search: CapSearchState) {
        if search.phase == CapSearchPhase::Up && search.candidate() > RfpllCapacitorCode::MAX {
            self.finish_cap_phase(search);
        } else {
            self.step = RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(search));
        }
    }

    fn finish_cap_phase(&mut self, search: CapSearchState) {
        if search.phase == CapSearchPhase::Down {
            self.continue_cap_search(CapSearchState {
                phase: CapSearchPhase::Up,
                phase_attempts: 0,
                boundaries: 0,
                ..search
            });
        } else {
            let final_cap = if search.accepted == 0 {
                search.initial
            } else {
                search.sum / u16::from(search.accepted)
            };
            self.step = RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Final {
                initial: search.initial,
                final_cap,
                accepted: search.accepted,
                lock_observed: search.lock_observed,
            });
        }
    }

    pub fn advance(
        &mut self,
        completion: RfpllFrequencyCompletion,
    ) -> Result<(), RfpllFrequencyTransitionError> {
        let action = self.action();
        self.step = match (self.step, completion) {
            (
                RfpllFrequencyStep::ChannelStart,
                RfpllFrequencyCompletion::ChannelSwitchStarted {
                    frequency_index,
                    crystal_selector,
                },
            ) if frequency_index
                == Self::programmed_frequency(self.request).wrapping_sub(2_400) as u8
                && crystal_selector == self.request.crystal_selector =>
            {
                RfpllFrequencyStep::ChannelStartDelay
            }
            (RfpllFrequencyStep::ChannelStartDelay, RfpllFrequencyCompletion::DelayElapsed(1)) => {
                RfpllFrequencyStep::ChannelClear
            }
            (RfpllFrequencyStep::ChannelClear, RfpllFrequencyCompletion::ChannelSwitchCleared) => {
                RfpllFrequencyStep::ChannelSettleDelay
            }
            (
                RfpllFrequencyStep::ChannelSettleDelay,
                RfpllFrequencyCompletion::DelayElapsed(10),
            ) => RfpllFrequencyStep::ChannelReady { samples: 0 },
            (
                RfpllFrequencyStep::ChannelReady { samples },
                RfpllFrequencyCompletion::ChannelReadyObserved { ready },
            ) => {
                if ready {
                    RfpllFrequencyStep::ChannelNrx
                } else {
                    RfpllFrequencyStep::ChannelReady {
                        samples: samples.wrapping_add(1),
                    }
                }
            }
            (
                RfpllFrequencyStep::ChannelReady { samples },
                RfpllFrequencyCompletion::ChannelReadyTimedOut,
            ) => {
                RfpllFrequencyStep::Failed(RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
                    samples,
                })
            }
            (
                RfpllFrequencyStep::ChannelNrx,
                RfpllFrequencyCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == Self::programmed_frequency(self.request) => {
                RfpllFrequencyStep::Complete(RfpllFrequencyOutcome {
                    lock_observed: true,
                    initial_cap: 0,
                    final_cap: 0,
                    accepted_cap_samples: 0,
                })
            }
            (RfpllFrequencyStep::InitialWrite { index, sdm }, _)
                if Self::matches_write(action, completion) =>
            {
                if index == 2 {
                    RfpllFrequencyStep::SdmWrite { index: 0, sdm }
                } else {
                    RfpllFrequencyStep::InitialWrite {
                        index: index + 1,
                        sdm,
                    }
                }
            }
            (RfpllFrequencyStep::SdmWrite { index, sdm }, _)
                if Self::matches_write(action, completion) =>
            {
                if index == 5 {
                    RfpllFrequencyStep::RestartWrite(0)
                } else {
                    RfpllFrequencyStep::SdmWrite {
                        index: index + 1,
                        sdm,
                    }
                }
            }
            (RfpllFrequencyStep::RestartWrite(index), _)
                if Self::matches_write(action, completion) =>
            {
                if index == 3 {
                    RfpllFrequencyStep::LockDelay { attempt: 0 }
                } else {
                    RfpllFrequencyStep::RestartWrite(index + 1)
                }
            }
            (
                RfpllFrequencyStep::LockDelay { attempt },
                RfpllFrequencyCompletion::DelayElapsed(20),
            ) => RfpllFrequencyStep::LockRead { attempt },
            (
                RfpllFrequencyStep::LockRead { attempt },
                RfpllFrequencyCompletion::MaskedRead {
                    field: analog_registers::RFPLL_LOCK_STATUS,
                    value,
                },
            ) => {
                if value != 0 {
                    RfpllFrequencyStep::CapLowRead {
                        lock_observed: true,
                    }
                } else if attempt + 1 == LOCK_ATTEMPTS {
                    RfpllFrequencyStep::CapLowRead {
                        lock_observed: false,
                    }
                } else {
                    RfpllFrequencyStep::LockDelay {
                        attempt: attempt + 1,
                    }
                }
            }
            (
                RfpllFrequencyStep::CapLowRead { lock_observed },
                RfpllFrequencyCompletion::ByteRead {
                    address: completed,
                    value,
                },
            ) if completed == analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW => {
                RfpllFrequencyStep::CapHighRead {
                    low: value,
                    lock_observed,
                }
            }
            (
                RfpllFrequencyStep::CapHighRead { low, lock_observed },
                RfpllFrequencyCompletion::MaskedRead {
                    field: analog_registers::RFPLL_CALIBRATED_CAPACITOR_HIGH,
                    value,
                },
            ) => RfpllFrequencyStep::EnableCapSearch {
                initial: u16::from(low) | (u16::from(value) << 8),
                lock_observed,
            },
            (
                RfpllFrequencyStep::EnableCapSearch {
                    initial,
                    lock_observed,
                },
                _,
            ) if Self::matches_write(action, completion) => {
                RfpllFrequencyStep::CapWriteLow(CapWriteContinuation::Search(CapSearchState {
                    initial,
                    phase: CapSearchPhase::Down,
                    phase_attempts: 0,
                    boundaries: 0,
                    accepted: 0,
                    sum: 0,
                    lock_observed,
                }))
            }
            (RfpllFrequencyStep::CapWriteLow(continuation), _)
                if Self::matches_write(action, completion) =>
            {
                RfpllFrequencyStep::CapWriteHigh(continuation)
            }
            (RfpllFrequencyStep::CapWriteHigh(continuation), _)
                if Self::matches_write(action, completion) =>
            {
                RfpllFrequencyStep::CapDelay(continuation)
            }
            (
                RfpllFrequencyStep::CapDelay(continuation),
                RfpllFrequencyCompletion::DelayElapsed(5),
            ) => match continuation {
                CapWriteContinuation::Search(search) => RfpllFrequencyStep::CapStatusRead(search),
                CapWriteContinuation::Final {
                    initial,
                    final_cap,
                    accepted,
                    lock_observed,
                } => RfpllFrequencyStep::Complete(RfpllFrequencyOutcome {
                    lock_observed,
                    initial_cap: initial,
                    final_cap,
                    accepted_cap_samples: accepted,
                }),
            },
            (
                RfpllFrequencyStep::CapStatusRead(mut search),
                RfpllFrequencyCompletion::MaskedRead {
                    field: analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS,
                    value,
                },
            ) => {
                if value == CAP_STATUS_ACCEPTED {
                    search.sum += RfpllCapacitorCode::programmed(search.candidate()).get();
                    search.accepted = search.accepted.wrapping_add(1);
                } else if matches!(
                    (search.phase, value),
                    (CapSearchPhase::Down, CAP_STATUS_INCREASE)
                        | (CapSearchPhase::Up, CAP_STATUS_DECREASE)
                ) {
                    search.boundaries += 1;
                }
                search.phase_attempts += 1;
                if search.phase_attempts == CAP_SEARCH_SAMPLES_PER_DIRECTION
                    || search.boundaries == CAP_SEARCH_BOUNDARIES
                {
                    self.finish_cap_phase(search);
                } else {
                    self.continue_cap_search(search);
                }
                return Ok(());
            }
            (RfpllFrequencyStep::Complete(_), _) | (RfpllFrequencyStep::Failed(_), _) => {
                return Err(RfpllFrequencyTransitionError::AlreadyComplete);
            }
            _ => return Err(RfpllFrequencyTransitionError::WrongCompletion),
        };
        Ok(())
    }

    #[cfg(feature = "validation-probes")]
    pub const fn request(self) -> RfpllFrequencyRequest {
        self.request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllFrequencyBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable owner of one RFPLL PHY-I2C operation.
///
/// The complete outer action is retained until the transaction finishes so
/// two adjacent writes to the same byte cannot exchange completions.
#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyI2cBinding {
    outer_action: RfpllFrequencyAction,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl RfpllFrequencyI2cBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        let request = match action {
            RfpllFrequencyAction::WriteMasked { field, value } => {
                crate::calibration::cold::PhyColdI2cRequest::write_field(field, value)
            }
            RfpllFrequencyAction::WriteByte { address, value } => {
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value)
            }
            RfpllFrequencyAction::ReadMasked { field } => {
                crate::calibration::cold::PhyColdI2cRequest::read_field(field)
            }
            RfpllFrequencyAction::ReadByte { address } => {
                crate::calibration::cold::PhyColdI2cRequest::read_byte(address)
            }
            RfpllFrequencyAction::StartChannelSwitch { .. }
            | RfpllFrequencyAction::ClearChannelSwitch
            | RfpllFrequencyAction::ReadChannelReady { .. }
            | RfpllFrequencyAction::ConfigureNrx { .. }
            | RfpllFrequencyAction::DelayMicros(_)
            | RfpllFrequencyAction::Complete(_)
            | RfpllFrequencyAction::Failed(_) => {
                return Err(RfpllFrequencyBindingError::UnsupportedAction);
            }
        };
        Ok(Self {
            outer_action: action,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<RfpllFrequencyCompletion, RfpllFrequencyBindingError> {
        let crate::calibration::cold::PhyColdI2cAction::Complete(outcome) =
            self.transaction.action()
        else {
            return Err(RfpllFrequencyBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                RfpllFrequencyAction::WriteMasked { field, .. },
                crate::calibration::cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == field.address() => {
                Ok(RfpllFrequencyCompletion::MaskedWrite { field })
            }
            (
                RfpllFrequencyAction::WriteByte { address, .. },
                crate::calibration::cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(RfpllFrequencyCompletion::ByteWrite { address }),
            (
                RfpllFrequencyAction::ReadMasked { field },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == field.address() => {
                Ok(RfpllFrequencyCompletion::MaskedRead { field, value })
            }
            (
                RfpllFrequencyAction::ReadByte { address },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(RfpllFrequencyCompletion::ByteRead { address, value }),
            _ => Err(RfpllFrequencyBindingError::UnexpectedOutcome),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyTimerBinding {
    micros: u32,
}

impl RfpllFrequencyTimerBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        match action {
            RfpllFrequencyAction::DelayMicros(micros) => Ok(Self { micros }),
            _ => Err(RfpllFrequencyBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> RfpllFrequencyCompletion {
        RfpllFrequencyCompletion::DelayElapsed(self.micros)
    }
}

/// Non-cloneable owner of one finite fast-channel MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct RfpllFrequencyMmioBinding {
    action: RfpllFrequencyAction,
}

impl RfpllFrequencyMmioBinding {
    pub fn new(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        match action {
            RfpllFrequencyAction::StartChannelSwitch { .. }
            | RfpllFrequencyAction::ClearChannelSwitch
            | RfpllFrequencyAction::ReadChannelReady { .. }
            | RfpllFrequencyAction::ConfigureNrx { .. } => Ok(Self { action }),
            _ => Err(RfpllFrequencyBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> RfpllFrequencyAction {
        self.action
    }

    /// Execute exactly one MMIO edge from the ROM fast-channel path.
    ///
    /// # Safety
    ///
    /// The caller must hold the unique radio-register owner and must not run
    /// another PHY frequency transition until the returned completion has
    /// been consumed.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> RfpllFrequencyCompletion {
        match self.action {
            RfpllFrequencyAction::StartChannelSwitch {
                frequency_index,
                crystal_selector,
            } => {
                oer_esp32s31_hal::phy::frequency::start_channel_switch(registers, frequency_index);
                RfpllFrequencyCompletion::ChannelSwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            RfpllFrequencyAction::ClearChannelSwitch => {
                oer_esp32s31_hal::phy::frequency::clear_channel_switch(registers);
                RfpllFrequencyCompletion::ChannelSwitchCleared
            }
            RfpllFrequencyAction::ReadChannelReady { .. } => {
                RfpllFrequencyCompletion::ChannelReadyObserved {
                    ready: oer_esp32s31_hal::phy::frequency::sample_frequency_ready(registers),
                }
            }
            RfpllFrequencyAction::ConfigureNrx { frequency_mhz } => {
                oer_esp32s31_hal::phy::frequency::configure_nrx_frequency(
                    registers,
                    u32::from(frequency_mhz),
                );
                RfpllFrequencyCompletion::NrxConfigured { frequency_mhz }
            }
            _ => unreachable!(),
        }
    }
}

/// Exhaustive lowering for every non-terminal RFPLL frequency action.
#[derive(Debug, Eq, PartialEq)]
pub enum RfpllFrequencyExternalBinding {
    Mmio(RfpllFrequencyMmioBinding),
    I2c(RfpllFrequencyI2cBinding),
    Timer(RfpllFrequencyTimerBinding),
}

impl RfpllFrequencyExternalBinding {
    pub fn lower(action: RfpllFrequencyAction) -> Result<Self, RfpllFrequencyBindingError> {
        if let Ok(binding) = RfpllFrequencyMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = RfpllFrequencyI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = RfpllFrequencyTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(RfpllFrequencyBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
