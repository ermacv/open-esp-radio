//! Rust-owned ESP32-S31 channel-programming transition.
//!
//! The pinned root is `libphy.a[phy_rfpll.o]::phy_chip_set_chan`, size
//! `0x122`.  Its qualified Wi-Fi AP/STA path reaches only channels 1 through
//! 13.  The two vendor PHY-I2C critical-section callbacks are single `ret`
//! instructions in the final image and are intentionally absent: the unique
//! Rust radio owner serializes the complete transition.
//!
//! The rev0 ROM fast-frequency child used for 2.4-GHz channels contains a
//! one-microsecond delay, a ten-microsecond delay and an unbounded poll of
//! `FREQUENCY_PARAMETER_1_STATUS.FREQUENCY_READY`. Rust represents both delays
//! and every readiness sample
//! as caller-completed actions.  Because no completion interrupt for that bit
//! is proved by the available PAC/SVD or ROM symbols, the outer async owner
//! may deliver further samples; it must deliver `FrequencyReadyTimedOut` at
//! its finite deadline.  No transition method spins, sleeps or self-wakes.

use crate::analog::{
    i2c::{PhyI2cAddress, analog_registers},
    pbus::{PhyForceTxRxAction, PhyForceTxRxCompletion, PhyForceTxRxDepth, PhyForceTxRxTransition},
    temperature::{
        PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureFailure,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
};

const TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;

// ESP32-S31 Wi-Fi gain profile consumed by `phy_wifi_get_tx_tab_new` in
// esp-phy-lib 20f1db053a0e6cb9f1c09d255c43bf42483041d0, libphy.a SHA-256
// 116c7c3a55e2ddfd7937edad2667b5d64717f1967519a0201ca6354246f0b1a2.
// phy_tx_gain.o .rodata offsets 0x6c/0x90/0xb4: 18 little-endian halfwords
// each. LOW/MID are RF/baseband settings; HIGH contains signed gain reference
// values used for interval selection and residual digital correction. Physical
// gain units are not established. These are Wi-Fi coefficients, not BT/154.
// Verification executes the real current callback, never a copied oracle table.
const WIFI_TX_GAIN_TABLE_LOW: [u16; 18] = [
    0x003f, 0x0037, 0x002f, 0x0027, 0x0027, 0x001f, 0x0017, 0x000f, 0x000f, 0x000d, 0x000c, 0x0007,
    0x0006, 0x0005, 0x0004, 0x0003, 0x0002, 0x0001,
];
const WIFI_TX_GAIN_TABLE_MID: [u16; 18] = [
    0x0100, 0x0100, 0x0100, 0x0100, 0x0080, 0x0080, 0x0080, 0x0080, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const WIFI_TX_GAIN_TABLE_HIGH: [u16; 18] = [
    0x001b, 0x0018, 0x0015, 0x000e, 0x0006, 0x0000, 0xfff6, 0xffe9, 0xffe1, 0xffd7, 0xffd0, 0xffc9,
    0xffc4, 0xffbe, 0xffb8, 0xffb0, 0xffa5, 0xff97,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelParameters {
    pub frequency_offset: i16,
    pub crystal_selector: u8,
    /// The qualified basic AP/STA profile requires this vendor option to be
    /// disabled. Channel 14 is rejected independently.
    pub channel_14_mic_enabled: bool,
    pub tx_gain_skip_publication: bool,
    pub tx_gain_seed: [u32; 6],
    pub tx_gain_config: u16,
    pub tx_gain_curve: [u8; 6],
    pub tx_gain_correction: i8,
    pub tx_gain_base: u8,
    /// Added to the base with signed-byte wrapping before gain calculation.
    /// Independent of Bluetooth's subtractive attenuation; physical units unknown.
    pub tx_gain_adjustment: i8,
    /// Former `phy_param[0xdc..=0xe1]`. Only bytes 0, 2 and 4 are selected
    /// by complete ROM `phy_set_txcap_reg`.
    pub tx_capacitance: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelRequest {
    /// A channel number or frequency in MHz, exactly as accepted by the
    /// pinned root.
    pub channel_or_frequency: u16,
    pub cbw: u8,
    pub parameters: PhyChipChannelParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainRequest {
    pub channel: u16,
    pub calibration_curve: [u8; 6],
    pub correction: i8,
    pub base_and_delta: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainImage {
    pub seed: [u32; 6],
    pub output_32: [u32; 8],
    pub output_64: [u32; 16],
    pub output_72: [u32; 16],
    pub config: u16,
}

const fn interpolate_tx_gain_curve(channel: u16, curve: [u8; 6]) -> i8 {
    let channel_index = channel.wrapping_sub(1);
    let value = if channel_index <= 5 {
        let low = curve[0] as i8 as i32;
        let high = curve[1] as i8 as i32;
        (high - low) * channel_index as i32 / 5 + low
    } else if channel_index <= 10 {
        let low = curve[1] as i8 as i32;
        let high = curve[2] as i8 as i32;
        (high - low) * (channel_index - 5) as i32 / 5 + low
    } else {
        curve[2] as i8 as i32 + 2
    };
    value as i8
}

const fn select_tx_gain_index(mut index: u8, target: i16) -> u8 {
    let mut iteration = 0;
    while iteration != WIFI_TX_GAIN_TABLE_HIGH.len() {
        let current = WIFI_TX_GAIN_TABLE_HIGH[index as usize] as i16;
        if target < current {
            if index as usize == WIFI_TX_GAIN_TABLE_HIGH.len() - 1 {
                break;
            }
            index += 1;
            let next = WIFI_TX_GAIN_TABLE_HIGH[index as usize] as i16;
            if target >= next {
                break;
            }
        } else {
            if index == 0 {
                break;
            }
            let previous = WIFI_TX_GAIN_TABLE_HIGH[index as usize - 1] as i16;
            if target < previous {
                break;
            }
            index -= 1;
        }
        iteration += 1;
    }
    index
}

const fn write_packed_byte(words: &mut [u32; 8], index: usize, value: u8) {
    let word = index >> 2;
    let shift = (index & 3) << 3;
    words[word] = (words[word] & !(0xff_u32 << shift)) | ((value as u32) << shift);
}

const fn write_packed_halfword<const N: usize>(words: &mut [u32; N], index: usize, value: u16) {
    let word = index >> 1;
    let shift = (index & 1) << 4;
    words[word] = (words[word] & !(0xffff_u32 << shift)) | ((value as u32) << shift);
}

/// Current S31 Wi-Fi coefficient profile and pure translation of rev0 ROM
/// `phy_wifi_get_tx_gain` (`0x2f826ff8`,
/// 258 bytes), including its `phy_set_chan_cal_interp` and
/// `phy_get_tx_gain_value` children.
///
/// The debug-only `ets_printf` branch is omitted. The reference caller always
/// supplies mode zero, and formatting is outside the radio algorithm. All
/// inputs and outputs are ordinary values; there is no MMIO, ROM ABI, global
/// state, allocation, callback, or wait.
pub const fn calculate_wifi_tx_gain(request: PhyWifiTxGainRequest) -> PhyWifiTxGainImage {
    let interpolation = interpolate_tx_gain_curve(request.channel, request.calibration_curve);
    let mut target = request.base_and_delta as i16 + 0x54 - request.correction as i16;
    let mut table_index = 0_u8;
    let mut image = PhyWifiTxGainImage {
        seed: [0; 6],
        output_32: [0; 8],
        output_64: [0; 16],
        output_72: [0; 16],
        config: 0,
    };
    let mut output_index = 0;
    while output_index != 32 {
        table_index = select_tx_gain_index(table_index, target);
        let table_index_usize = table_index as usize;
        let residual = target.wrapping_sub(WIFI_TX_GAIN_TABLE_HIGH[table_index_usize] as i16);
        write_packed_byte(
            &mut image.output_32,
            output_index,
            residual.wrapping_sub(interpolation as i16) as u8,
        );
        write_packed_halfword(
            &mut image.output_64,
            output_index,
            WIFI_TX_GAIN_TABLE_MID[table_index_usize],
        );
        write_packed_halfword(
            &mut image.output_72,
            output_index,
            WIFI_TX_GAIN_TABLE_LOW[table_index_usize],
        );
        target = target.wrapping_sub(4);
        output_index += 1;
    }
    image
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelOutcome {
    pub channel: u16,
    pub frequency_mhz: u16,
    pub cbw: u8,
    pub init_complete: bool,
    pub temperature: PhyTemperatureOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelFailure {
    UnsupportedChannel(u16),
    UnsupportedFrequency(u16),
    Channel14MicEnabled,
    Temperature(PhyTemperatureFailure),
    FrequencyReadyTimedOut { samples: u32 },
    I2cTimedOut { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelDelay {
    FrequencyStartPulse,
    FrequencySettle,
    /// Settle after one force-TX/RX write of the given direction and phase.
    ForceTxRx {
        enabled: bool,
        completed_phase: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelI2cPhase {
    ProgramTxCap,
    CaptureTxCap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelAction {
    SetAgc {
        enabled: bool,
    },
    /// One force-TX/RX write selected by the `phy_force_txrx_off_new` depth.
    ConfigureForceTxRx {
        enabled: bool,
        phase: u8,
    },
    SetBbpllCalibration {
        enabled: bool,
    },
    Temperature(PhyTemperatureAction),
    StartFrequencySwitch {
        frequency_index: u8,
        crystal_selector: u8,
    },
    DelayMicros {
        phase: PhyChipChannelDelay,
        micros: u32,
    },
    ClearFrequencySwitch,
    AwaitFrequencyReadyEdge {
        samples: u32,
    },
    ConfigureNrx {
        frequency_mhz: u16,
    },
    ConfigureBssCbw {
        cbw: u8,
    },
    ConfigureRxCompensation,
    WriteI2c {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    CalculateTxGain(PhyWifiTxGainRequest),
    PublishTxGain(PhyWifiTxGainImage),
    ReadI2c {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
    },
    PublishTxCapCommandMemory {
        value: u8,
    },
    ConfigureChannelCbw {
        cbw: u8,
    },
    ClearDcMemory,
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelCompletion {
    AgcSet {
        enabled: bool,
    },
    ForceTxRxConfigured {
        enabled: bool,
        phase: u8,
    },
    BbpllCalibrationSet {
        enabled: bool,
    },
    Temperature(PhyTemperatureCompletion),
    FrequencySwitchStarted {
        frequency_index: u8,
        crystal_selector: u8,
    },
    DelayElapsed {
        phase: PhyChipChannelDelay,
        micros: u32,
    },
    FrequencySwitchCleared,
    FrequencyReadyObserved {
        ready: bool,
    },
    FrequencyReadyTimedOut,
    NrxConfigured {
        frequency_mhz: u16,
    },
    BssCbwConfigured {
        cbw: u8,
    },
    RxCompensationConfigured,
    I2cWriteCompleted {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    I2cReadCompleted {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
        value: u8,
    },
    I2cTimedOut {
        phase: PhyChipChannelI2cPhase,
        address: PhyI2cAddress,
    },
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    I2cDeadlineExceeded {
        address: PhyI2cAddress,
    },
    TxGainCalculated {
        request: PhyWifiTxGainRequest,
        image: PhyWifiTxGainImage,
    },
    TxGainPublished,
    TxCapCommandMemoryPublished {
        value: u8,
    },
    ChannelCbwConfigured {
        cbw: u8,
    },
    DcMemoryCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupContinuation {
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    DisableAgc,
    ForceTxRx(PhyForceTxRxTransition),
    EnableBbpll,
    Temperature(PhyTemperatureTransition),
    StartFrequencySwitch,
    FrequencyStartDelay,
    ClearFrequencySwitch,
    FrequencySettleDelay,
    AwaitFrequencyReady {
        samples: u32,
    },
    ConfigureNrxFirst,
    ConfigureBssCbw,
    ConfigureRxCompensationFirst,
    ProgramTxCap,
    ConfigureNrxSecond,
    CalculateTxGain,
    PublishTxGain(PhyWifiTxGainImage),
    /// The TX-gain child's release, which forces again at depth one.
    ReassertTxRx(PhyForceTxRxTransition),
    CaptureTxCap,
    PublishTxCapCommandMemory {
        value: u8,
    },
    ConfigureChannelCbw,
    ConfigureRxCompensationSecond,
    DisableBbpll(CleanupContinuation),
    ReleaseTxRx(PhyForceTxRxTransition, CleanupContinuation),
    ClearDcMemory(CleanupContinuation),
    EnableAgc(CleanupContinuation),
    Complete(PhyChipChannelOutcome),
    Failed(PhyChipChannelFailure),
}

/// Exact pure translation of rev0 ROM `phy_chan_to_freq`.
pub const fn channel_to_frequency(channel: u16) -> u16 {
    if channel > 14 {
        channel
    } else if channel == 14 {
        2_484
    } else {
        2_407_u16.wrapping_add(channel.wrapping_mul(5))
    }
}

/// Exact pure translation of rev0 ROM `phy_mhz2ieee`.
pub const fn frequency_to_channel(frequency_mhz: u16) -> u16 {
    if frequency_mhz == 2_484 {
        14
    } else if frequency_mhz <= 2_483 {
        ((frequency_mhz.wrapping_sub(2_407) / 5) as u8 as i8) as u16
    } else {
        (((frequency_mhz.wrapping_sub(5_008) / 20).wrapping_add(15)) as u8 as i8) as u16
    }
}

const fn normalized_channel(channel_or_frequency: u16) -> u16 {
    if channel_or_frequency > 2_411 {
        frequency_to_channel(channel_or_frequency)
    } else {
        channel_or_frequency
    }
}

const fn tx_cap_value(channel: u16, capacitance: [u8; 6]) -> u8 {
    let selected = if channel <= 3 {
        capacitance[0]
    } else if channel <= 8 {
        capacitance[2]
    } else {
        capacitance[4]
    };
    selected | 0xc0
}

/// Recreate the two-nibble TX-cap image restored by retained wake.
///
/// Complete ROM `phy_set_txcap_reset` selects the low/high pair for the
/// current channel group. This deliberately differs from [`tx_cap_value`],
/// whose upper two bits select the ordinary channel-programming command.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) const fn retained_tx_cap_value(channel: u16, capacitance: [u8; 6]) -> u8 {
    let index = if channel <= 3 {
        0
    } else if channel <= 8 {
        2
    } else {
        4
    };
    capacitance[index] | capacitance[index + 1].wrapping_shl(4)
}

const fn force_txrx_action(child: PhyForceTxRxTransition) -> PhyChipChannelAction {
    match child.action() {
        PhyForceTxRxAction::Configure { enabled, phase } => {
            PhyChipChannelAction::ConfigureForceTxRx { enabled, phase }
        }
        PhyForceTxRxAction::DelayMicros {
            enabled,
            completed_phase,
            micros,
        } => PhyChipChannelAction::DelayMicros {
            phase: PhyChipChannelDelay::ForceTxRx {
                enabled,
                completed_phase,
            },
            micros,
        },
        // The parent leaves a force step as soon as its child completes.
        PhyForceTxRxAction::Complete { .. } => unreachable!(),
    }
}

/// Advance a force-TX/RX child; `true` once both writes and settles completed.
fn advance_force_txrx(
    child: &mut PhyForceTxRxTransition,
    completion: PhyChipChannelCompletion,
) -> Result<bool, PhyChipChannelTransitionError> {
    let completion = match completion {
        PhyChipChannelCompletion::ForceTxRxConfigured { enabled, phase } => {
            PhyForceTxRxCompletion::Configured { enabled, phase }
        }
        PhyChipChannelCompletion::DelayElapsed {
            phase:
                PhyChipChannelDelay::ForceTxRx {
                    enabled,
                    completed_phase,
                },
            micros,
        } => PhyForceTxRxCompletion::DelayElapsed {
            enabled,
            completed_phase,
            micros,
        },
        _ => return Err(PhyChipChannelTransitionError::WrongCompletion),
    };
    child
        .advance(completion)
        .map_err(|_| PhyChipChannelTransitionError::WrongCompletion)?;
    Ok(matches!(
        child.action(),
        PhyForceTxRxAction::Complete { .. }
    ))
}

impl PhyChipChannelRequest {
    pub const fn validate(self) -> Result<(), PhyChipChannelFailure> {
        let channel = normalized_channel(self.channel_or_frequency);
        if self.parameters.channel_14_mic_enabled {
            return Err(PhyChipChannelFailure::Channel14MicEnabled);
        }
        if channel == 0 || channel > 13 {
            return Err(PhyChipChannelFailure::UnsupportedChannel(channel));
        }
        let frequency = channel_to_frequency(self.channel_or_frequency);
        let index = frequency.wrapping_sub(2_400);
        if index > 84 {
            return Err(PhyChipChannelFailure::UnsupportedFrequency(frequency));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChipChannelTransition {
    request: PhyChipChannelRequest,
    /// Force-TX/RX nesting count held by the caller.
    depth: PhyForceTxRxDepth,
    channel: u16,
    frequency_mhz: u16,
    temperature: Option<PhyTemperatureOutcome>,
    step: Step,
}

impl PhyChipChannelTransition {
    /// Outermost channel programming: no caller holds a force-TX/RX level.
    pub const fn new(request: PhyChipChannelRequest) -> Self {
        Self::at_depth(request, PhyForceTxRxDepth::OUTERMOST)
    }

    /// Channel programming inside callers holding `depth` force-TX/RX levels.
    /// The root's own pair and the nested TX-gain child's pair run the
    /// sequences [`PhyForceTxRxDepth`] selects.
    pub const fn at_depth(request: PhyChipChannelRequest, depth: PhyForceTxRxDepth) -> Self {
        let channel = normalized_channel(request.channel_or_frequency);
        let frequency_mhz = channel_to_frequency(request.channel_or_frequency);
        let step = match request.validate() {
            Ok(()) => Step::DisableAgc,
            Err(failure) => Step::Failed(failure),
        };
        Self {
            request,
            depth,
            channel,
            frequency_mhz,
            temperature: None,
            step,
        }
    }

    const fn frequency_index(self) -> u8 {
        self.frequency_mhz.wrapping_sub(2_400) as u8
    }

    const fn outcome(self) -> PhyChipChannelOutcome {
        let Some(temperature) = self.temperature else {
            panic!("channel transition completed without a temperature outcome");
        };
        PhyChipChannelOutcome {
            channel: self.channel,
            frequency_mhz: self.frequency_mhz,
            cbw: self.request.cbw,
            init_complete: self.request.cbw != 0,
            temperature,
        }
    }

    const fn tx_gain_request(self) -> PhyWifiTxGainRequest {
        PhyWifiTxGainRequest {
            channel: self.channel,
            calibration_curve: self.request.parameters.tx_gain_curve,
            correction: self.request.parameters.tx_gain_correction,
            base_and_delta: self
                .request
                .parameters
                .tx_gain_base
                .wrapping_add(self.request.parameters.tx_gain_adjustment as u8)
                as i8,
        }
    }

    pub const fn action(self) -> PhyChipChannelAction {
        match self.step {
            Step::DisableAgc => PhyChipChannelAction::SetAgc { enabled: false },
            Step::ForceTxRx(child) | Step::ReassertTxRx(child) | Step::ReleaseTxRx(child, _) => {
                force_txrx_action(child)
            }
            Step::EnableBbpll => PhyChipChannelAction::SetBbpllCalibration { enabled: true },
            Step::Temperature(transition) => PhyChipChannelAction::Temperature(transition.action()),
            Step::StartFrequencySwitch => PhyChipChannelAction::StartFrequencySwitch {
                frequency_index: self.frequency_index(),
                crystal_selector: self.request.parameters.crystal_selector,
            },
            Step::FrequencyStartDelay => PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencyStartPulse,
                micros: 1,
            },
            Step::ClearFrequencySwitch => PhyChipChannelAction::ClearFrequencySwitch,
            Step::FrequencySettleDelay => PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::FrequencySettle,
                micros: 10,
            },
            Step::AwaitFrequencyReady { samples } => {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples }
            }
            Step::ConfigureNrxFirst => PhyChipChannelAction::ConfigureNrx {
                frequency_mhz: self.frequency_mhz,
            },
            // The pinned root accepts off-grid MHz inputs. Its first NRX
            // update receives that raw input, while the second update follows
            // TX-cap programming and receives the canonical frequency of the
            // truncated channel number.
            Step::ConfigureNrxSecond => PhyChipChannelAction::ConfigureNrx {
                frequency_mhz: channel_to_frequency(self.channel),
            },
            Step::ConfigureBssCbw => PhyChipChannelAction::ConfigureBssCbw {
                cbw: self.request.cbw,
            },
            Step::ConfigureRxCompensationFirst => PhyChipChannelAction::ConfigureRxCompensation,
            Step::ProgramTxCap => PhyChipChannelAction::WriteI2c {
                phase: PhyChipChannelI2cPhase::ProgramTxCap,
                address: TX_CAP_ADDRESS,
                value: tx_cap_value(self.channel, self.request.parameters.tx_capacitance),
            },
            Step::CalculateTxGain => PhyChipChannelAction::CalculateTxGain(self.tx_gain_request()),
            Step::PublishTxGain(image) => PhyChipChannelAction::PublishTxGain(image),
            Step::CaptureTxCap => PhyChipChannelAction::ReadI2c {
                phase: PhyChipChannelI2cPhase::CaptureTxCap,
                address: TX_CAP_ADDRESS,
            },
            Step::PublishTxCapCommandMemory { value } => {
                PhyChipChannelAction::PublishTxCapCommandMemory { value }
            }
            Step::ConfigureChannelCbw => PhyChipChannelAction::ConfigureChannelCbw {
                cbw: self.request.cbw,
            },
            Step::ConfigureRxCompensationSecond => PhyChipChannelAction::ConfigureRxCompensation,
            Step::DisableBbpll(_) => PhyChipChannelAction::SetBbpllCalibration { enabled: false },
            Step::ClearDcMemory(_) => PhyChipChannelAction::ClearDcMemory,
            Step::EnableAgc(_) => PhyChipChannelAction::SetAgc { enabled: true },
            Step::Complete(outcome) => PhyChipChannelAction::Complete(outcome),
            Step::Failed(failure) => PhyChipChannelAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyChipChannelCompletion,
    ) -> Result<(), PhyChipChannelTransitionError> {
        self.step = match (self.step, completion) {
            (Step::DisableAgc, PhyChipChannelCompletion::AgcSet { enabled: false }) => {
                match self.depth.enter() {
                    Some(child) => Step::ForceTxRx(child),
                    None => Step::EnableBbpll,
                }
            }
            (Step::ReassertTxRx(mut child), completion) => {
                if advance_force_txrx(&mut child, completion)? {
                    Step::CaptureTxCap
                } else {
                    Step::ReassertTxRx(child)
                }
            }
            (Step::ForceTxRx(mut child), completion) => {
                if advance_force_txrx(&mut child, completion)? {
                    Step::EnableBbpll
                } else {
                    Step::ForceTxRx(child)
                }
            }
            (Step::ReleaseTxRx(mut child, continuation), completion) => {
                if advance_force_txrx(&mut child, completion)? {
                    Step::ClearDcMemory(continuation)
                } else {
                    Step::ReleaseTxRx(child, continuation)
                }
            }
            (
                Step::EnableBbpll,
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled: true },
            ) => Step::Temperature(PhyTemperatureTransition::new()),
            (
                Step::Temperature(mut transition),
                PhyChipChannelCompletion::Temperature(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyChipChannelTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTemperatureAction::Complete(outcome) => {
                        self.temperature = Some(outcome);
                        Step::StartFrequencySwitch
                    }
                    PhyTemperatureAction::Failed(failure) => {
                        let continuation = CleanupContinuation::Failed(
                            PhyChipChannelFailure::Temperature(failure),
                        );
                        Step::DisableBbpll(continuation)
                    }
                    _ => Step::Temperature(transition),
                }
            }
            (Step::Temperature(_), PhyChipChannelCompletion::I2cDeadlineExceeded { address }) => {
                Step::DisableBbpll(CleanupContinuation::Failed(
                    PhyChipChannelFailure::I2cTimedOut { address },
                ))
            }
            (
                Step::StartFrequencySwitch,
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                },
            ) if frequency_index == self.frequency_index()
                && crystal_selector == self.request.parameters.crystal_selector =>
            {
                Step::FrequencyStartDelay
            }
            (
                Step::FrequencyStartDelay,
                PhyChipChannelCompletion::DelayElapsed {
                    phase: PhyChipChannelDelay::FrequencyStartPulse,
                    micros: 1,
                },
            ) => Step::ClearFrequencySwitch,
            (Step::ClearFrequencySwitch, PhyChipChannelCompletion::FrequencySwitchCleared) => {
                Step::FrequencySettleDelay
            }
            (
                Step::FrequencySettleDelay,
                PhyChipChannelCompletion::DelayElapsed {
                    phase: PhyChipChannelDelay::FrequencySettle,
                    micros: 10,
                },
            ) => Step::AwaitFrequencyReady { samples: 0 },
            (
                Step::AwaitFrequencyReady { samples },
                PhyChipChannelCompletion::FrequencyReadyObserved { ready },
            ) => {
                if ready {
                    Step::ConfigureNrxFirst
                } else {
                    Step::AwaitFrequencyReady {
                        samples: samples.wrapping_add(1),
                    }
                }
            }
            (
                Step::AwaitFrequencyReady { samples },
                PhyChipChannelCompletion::FrequencyReadyTimedOut,
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::FrequencyReadyTimedOut { samples },
            )),
            (
                Step::ConfigureNrxFirst,
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == self.frequency_mhz => Step::ConfigureBssCbw,
            (Step::ConfigureBssCbw, PhyChipChannelCompletion::BssCbwConfigured { cbw })
                if cbw == self.request.cbw =>
            {
                Step::ConfigureRxCompensationFirst
            }
            (
                Step::ConfigureRxCompensationFirst,
                PhyChipChannelCompletion::RxCompensationConfigured,
            ) => Step::ProgramTxCap,
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cWriteCompleted {
                    phase: PhyChipChannelI2cPhase::ProgramTxCap,
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) if value == tx_cap_value(self.channel, self.request.parameters.tx_capacitance) => {
                Step::ConfigureNrxSecond
            }
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cTimedOut {
                    phase: PhyChipChannelI2cPhase::ProgramTxCap,
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::ProgramTxCap,
                PhyChipChannelCompletion::I2cDeadlineExceeded {
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::ConfigureNrxSecond,
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz },
            ) if frequency_mhz == channel_to_frequency(self.channel) => Step::CalculateTxGain,
            (
                Step::CalculateTxGain,
                PhyChipChannelCompletion::TxGainCalculated { request, mut image },
            ) if request == self.tx_gain_request() => {
                image.seed = self.request.parameters.tx_gain_seed;
                image.config = self.request.parameters.tx_gain_config;
                if self.request.parameters.tx_gain_skip_publication {
                    Step::CaptureTxCap
                } else {
                    Step::PublishTxGain(image)
                }
            }
            // `phy_wifi_set_tx_gain_new` brackets its publication with a
            // pair nested inside this root's pair.
            (Step::PublishTxGain(_), PhyChipChannelCompletion::TxGainPublished) => {
                match self.depth.nested().leave() {
                    Some(child) => Step::ReassertTxRx(child),
                    None => Step::CaptureTxCap,
                }
            }
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cReadCompleted {
                    phase: PhyChipChannelI2cPhase::CaptureTxCap,
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) => Step::PublishTxCapCommandMemory { value },
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cTimedOut {
                    phase: PhyChipChannelI2cPhase::CaptureTxCap,
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::CaptureTxCap,
                PhyChipChannelCompletion::I2cDeadlineExceeded {
                    address: TX_CAP_ADDRESS,
                },
            ) => Step::DisableBbpll(CleanupContinuation::Failed(
                PhyChipChannelFailure::I2cTimedOut {
                    address: TX_CAP_ADDRESS,
                },
            )),
            (
                Step::PublishTxCapCommandMemory { value },
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value: completed },
            ) if value == completed => Step::ConfigureChannelCbw,
            (Step::ConfigureChannelCbw, PhyChipChannelCompletion::ChannelCbwConfigured { cbw })
                if cbw == self.request.cbw =>
            {
                Step::ConfigureRxCompensationSecond
            }
            (
                Step::ConfigureRxCompensationSecond,
                PhyChipChannelCompletion::RxCompensationConfigured,
            ) => Step::DisableBbpll(CleanupContinuation::Complete(self.outcome())),
            (
                Step::DisableBbpll(continuation),
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled: false },
            ) => match self.depth.leave() {
                Some(child) => Step::ReleaseTxRx(child, continuation),
                None => Step::ClearDcMemory(continuation),
            },
            (Step::ClearDcMemory(continuation), PhyChipChannelCompletion::DcMemoryCleared) => {
                Step::EnableAgc(continuation)
            }
            (Step::EnableAgc(continuation), PhyChipChannelCompletion::AgcSet { enabled: true }) => {
                match continuation {
                    CleanupContinuation::Complete(outcome) => Step::Complete(outcome),
                    CleanupContinuation::Failed(failure) => Step::Failed(failure),
                }
            }
            (Step::Complete(_) | Step::Failed(_), _) => {
                return Err(PhyChipChannelTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyChipChannelTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelBindingError {
    NotDirectMmio,
}

/// Non-cloneable token for one finite channel-specific MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelMmioBinding {
    action: PhyChipChannelAction,
}

impl PhyChipChannelMmioBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelBindingError> {
        match action {
            PhyChipChannelAction::SetAgc { .. }
            | PhyChipChannelAction::ConfigureForceTxRx { .. }
            | PhyChipChannelAction::SetBbpllCalibration { .. }
            | PhyChipChannelAction::StartFrequencySwitch { .. }
            | PhyChipChannelAction::ClearFrequencySwitch
            | PhyChipChannelAction::AwaitFrequencyReadyEdge { .. }
            | PhyChipChannelAction::ConfigureNrx { .. }
            | PhyChipChannelAction::ConfigureBssCbw { .. }
            | PhyChipChannelAction::ConfigureRxCompensation
            | PhyChipChannelAction::PublishTxGain(_)
            | PhyChipChannelAction::PublishTxCapCommandMemory { .. }
            | PhyChipChannelAction::ConfigureChannelCbw { .. }
            | PhyChipChannelAction::ClearDcMemory => Ok(Self { action }),
            _ => Err(PhyChipChannelBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyChipChannelAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_channel_hal<P>(
        self,
        channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    ) -> PhyChipChannelCompletion {
        match self.action {
            PhyChipChannelAction::SetAgc { enabled } => {
                channel.set_agc_enabled(enabled);
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::ConfigureForceTxRx { enabled, phase } => {
                oer_esp32s31_hal::phy::pbus::configure_force_txrx(channel, enabled, phase);
                PhyChipChannelCompletion::ForceTxRxConfigured { enabled, phase }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                channel.set_bbpll_calibration_enabled(enabled);
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => {
                channel.start_frequency_switch(frequency_index);
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                channel.clear_frequency_switch();
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { .. } => {
                PhyChipChannelCompletion::FrequencyReadyObserved {
                    ready: channel.frequency_ready(),
                }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                channel.configure_nrx(frequency_mhz);
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                channel.configure_bss_cbw(cbw);
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                channel.configure_rx_compensation();
                PhyChipChannelCompletion::RxCompensationConfigured
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                crate::hardware::publish_phy_tx_gain_memory_channel(channel, false, image);
                PhyChipChannelCompletion::TxGainPublished
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                channel.publish_tx_cap(value);
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                channel.configure_channel_cbw(cbw);
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => {
                channel.clear_dc_memory();
                PhyChipChannelCompletion::DcMemoryCleared
            }
            _ => unreachable!(),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P>(
        self,
        _platform: &mut P,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyChipChannelCompletion {
        match self.action {
            PhyChipChannelAction::SetAgc { enabled } => {
                crate::hardware::set_phy_channel_agc(registers, enabled);
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::ConfigureForceTxRx { enabled, phase } => {
                oer_esp32s31_hal::phy::pbus::configure_force_txrx(registers, enabled, phase);
                PhyChipChannelCompletion::ForceTxRxConfigured { enabled, phase }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                oer_esp32s31_hal::phy::i2c::configure_bbpll_calibration(registers, enabled);
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => {
                oer_esp32s31_hal::phy::frequency::start_channel_switch(registers, frequency_index);
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                oer_esp32s31_hal::phy::frequency::clear_channel_switch(registers);
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { .. } => {
                PhyChipChannelCompletion::FrequencyReadyObserved {
                    ready: oer_esp32s31_hal::phy::frequency::sample_frequency_ready(registers),
                }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                oer_esp32s31_hal::phy::frequency::configure_nrx_frequency(
                    registers,
                    u32::from(frequency_mhz),
                );
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                oer_esp32s31_hal::phy::frequency::configure_bss_cbw(registers, cbw);
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                oer_esp32s31_hal::phy::agc::configure_rx_compensation(registers);
                PhyChipChannelCompletion::RxCompensationConfigured
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                crate::hardware::publish_phy_tx_gain_memory(registers, false, image);
                PhyChipChannelCompletion::TxGainPublished
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                oer_esp32s31_hal::phy::frequency::publish_tx_cap(registers, value);
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                oer_esp32s31_hal::phy::frequency::configure_channel_cbw(registers, u32::from(cbw));
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => {
                oer_esp32s31_hal::phy::agc::clear_dc_memory(registers);
                PhyChipChannelCompletion::DcMemoryCleared
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable token for the pure Rust TX-gain calculation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyWifiTxGainBinding {
    request: PhyWifiTxGainRequest,
}

impl PhyWifiTxGainBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelBindingError> {
        match action {
            PhyChipChannelAction::CalculateTxGain(request) => Ok(Self { request }),
            _ => Err(PhyChipChannelBindingError::NotDirectMmio),
        }
    }

    pub const fn request(&self) -> PhyWifiTxGainRequest {
        self.request
    }

    pub const fn execute(self) -> PhyChipChannelCompletion {
        PhyChipChannelCompletion::TxGainCalculated {
            request: self.request,
            image: calculate_wifi_tx_gain(self.request),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyChipChannelExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelTimerBinding {
    phase: PhyChipChannelDelay,
    micros: u32,
}

impl PhyChipChannelTimerBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        match action {
            PhyChipChannelAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyChipChannelExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyChipChannelCompletion {
        PhyChipChannelCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyChipChannelI2cBinding {
    outer_action: PhyChipChannelAction,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyChipChannelI2cBinding {
    pub fn new(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        let request = match action {
            PhyChipChannelAction::WriteI2c { address, value, .. } => {
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value)
            }
            PhyChipChannelAction::ReadI2c { address, .. } => {
                crate::calibration::cold::PhyColdI2cRequest::read_byte(address)
            }
            _ => return Err(PhyChipChannelExternalBindingError::UnsupportedAction),
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

    pub fn into_completion(
        self,
    ) -> Result<PhyChipChannelCompletion, PhyChipChannelExternalBindingError> {
        let crate::calibration::cold::PhyColdI2cAction::Complete(outcome) =
            self.transaction.action()
        else {
            return Err(PhyChipChannelExternalBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyChipChannelAction::WriteI2c {
                    phase,
                    address,
                    value,
                },
                crate::calibration::cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(PhyChipChannelCompletion::I2cWriteCompleted {
                phase,
                address,
                value,
            }),
            (
                PhyChipChannelAction::ReadI2c { phase, address },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(PhyChipChannelCompletion::I2cReadCompleted {
                phase,
                address,
                value,
            }),
            _ => Err(PhyChipChannelExternalBindingError::UnexpectedOutcome),
        }
    }

    pub const fn into_timeout_completion(self) -> PhyChipChannelCompletion {
        match self.outer_action {
            PhyChipChannelAction::WriteI2c { phase, address, .. }
            | PhyChipChannelAction::ReadI2c { phase, address } => {
                PhyChipChannelCompletion::I2cTimedOut { phase, address }
            }
            _ => unreachable!(),
        }
    }
}

/// Exhaustive lowering of every non-terminal channel action.
///
#[derive(Debug, Eq, PartialEq)]
pub enum PhyChipChannelExternalBinding {
    Mmio(PhyChipChannelMmioBinding),
    Temperature(crate::analog::temperature::PhyTemperatureExternalBinding),
    Timer(PhyChipChannelTimerBinding),
    I2c(PhyChipChannelI2cBinding),
    TxGain(PhyWifiTxGainBinding),
}

impl PhyChipChannelExternalBinding {
    pub fn lower(action: PhyChipChannelAction) -> Result<Self, PhyChipChannelExternalBindingError> {
        if let Ok(binding) = PhyChipChannelMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyChipChannelAction::Temperature(action) => {
                crate::analog::temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyChipChannelExternalBindingError::UnsupportedAction)
            }
            PhyChipChannelAction::DelayMicros { .. } => {
                PhyChipChannelTimerBinding::new(action).map(Self::Timer)
            }
            PhyChipChannelAction::WriteI2c { .. } | PhyChipChannelAction::ReadI2c { .. } => {
                PhyChipChannelI2cBinding::new(action).map(Self::I2c)
            }
            PhyChipChannelAction::CalculateTxGain(_) => PhyWifiTxGainBinding::new(action)
                .map(Self::TxGain)
                .map_err(|_| PhyChipChannelExternalBindingError::UnsupportedAction),
            PhyChipChannelAction::Complete(_) | PhyChipChannelAction::Failed(_) => {
                Err(PhyChipChannelExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyChipChannelExternalBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;
