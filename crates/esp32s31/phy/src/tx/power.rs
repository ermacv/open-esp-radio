//! Rust-owned ESP32-S31 TX power-control calibration.
//!
//! This module replaces the mandatory 154-byte archive root
//! `phy_tx_pwctrl_init`, its 396-byte archive child
//! `phy_tx_pwctrl_init_cal_new`, and the complete 462-byte ROM search
//! `phy_rfcal_pwrctrl`. Diagnostic formatting branches are intentionally
//! absent. Every tone/SAR measurement, RFPLL operation, PHY-I2C write, PBus
//! command and cleanup edge remains independently completed by the async
//! executor.

use crate::{
    analog::{
        i2c::{PhyI2cAddress, analog_registers},
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
            RfpllFrequencyRequest, RfpllFrequencyTransition,
        },
    },
    tx::calibration::{
        PhyToneSarAction, PhyToneSarCompletion, PhyToneSarFailure, PhyToneSarRequest,
        PhyToneSarTransition, PhyTxCalibrationEnvironment, PhyTxCalibrationEnvironmentAction,
        PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentFailure,
        PhyTxCalibrationEnvironmentTransition, PhyTxCalibrationParameters, phy_tx_power_db,
    },
};

/// Number of signed quarter-unit targets consumed by the rev0 rate mapping.
pub const PHY_TX_TARGET_POWER_COUNT: usize = 18;

/// Calibrated PHY gain-table indices for one MAC rate.
///
/// These are signed table indices after division of the PHY quarter-unit
/// target by four. They are not dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxTargetPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl PhyTxTargetPowerPair {
    pub const ZERO: Self = Self {
        primary: 0,
        alternate: 0,
    };
}

/// Rust-owned replacement for the target-power part of `phy_get_max_pwr`.
///
/// SOURCE\[`ROM_REV0_PHY_GET_MAX_PWR`]: complete rev0 ROM
/// `phy_get_max_pwr` (0x2f82_49fe), `phy_get_target_pwr` (0x2f82_4976),
/// `phy_wifi_get_target_power` (0x2f82_70fa), and `phy_rate_to_index`
/// (0x2f82_491e), recovered from `esp32s31_rev0_rom.elf`.
///
/// The profile owns the former `phy_param` inputs. It contains no pointer to
/// the cold state and never reads the vendor `phy_param` pointer cell or
/// `s_phy_get_max_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxTargetPowerProfile {
    maximum: i8,
    target: [i8; PHY_TX_TARGET_POWER_COUNT],
    regulatory_override: bool,
}

impl PhyTxTargetPowerProfile {
    pub(crate) const fn new(
        maximum: i8,
        target: [i8; PHY_TX_TARGET_POWER_COUNT],
        regulatory_override: bool,
    ) -> Self {
        Self {
            maximum,
            target,
            regulatory_override,
        }
    }

    /// Apply the upper-MAC transmit-power limit expressed in quarter-dBm.
    ///
    /// This is the source-owned equivalent of the limit installed by
    /// `esp_wifi_set_max_tx_power`: the PHY init-data ceiling and the runtime
    /// ceiling are both quarter-dBm values and the smaller one bounds every
    /// rate before `phy_get_max_pwr` converts it to the integer power code.
    ///
    /// SOURCE: `esp32s31_rev0_rom.elf::phy_get_target_pwr` clamps the
    /// signed per-rate target before `phy_get_max_pwr` arithmetic-shifts it by
    /// two; `libpp.a[hal_mac_tx.o]::hal_set_tx_pwr` publishes the
    /// resulting per-rate byte. The pinned ESP32-S31 vendor stack running on
    /// hardware confirms that a quarter-dBm limit of 20 produces table byte 5
    /// and `TX_Q0_POWER=0x0505_0005`.
    pub fn with_maximum_quarter_dbm(mut self, maximum_quarter_dbm: i8) -> Self {
        self.maximum = self.maximum.min(maximum_quarter_dbm);
        self
    }

    /// Produce the two calibrated indices returned by the ROM root for a rate.
    ///
    /// Codes 32..=40 map beyond the recovered 18-byte target array and are
    /// reserved by the current MAC tables. They fail closed to zero instead of
    /// indexing outside owned state as the ROM routine can.
    pub fn pair(&self, rate: u8) -> PhyTxTargetPowerPair {
        let Some(index) = rate_to_target_index(rate) else {
            return PhyTxTargetPowerPair::ZERO;
        };

        // With the cold profile's regulatory flag clear, complete
        // phy_get_chan_target_power clamps every signed target first to the
        // selected FCC byte. All four default bytes at ROM 0x2f84_834c are 84.
        //
        // The override branch selects country/FCC state not yet owned by the
        // open driver. Its strongest safe bound is the calibrated maximum.
        let target = |target_index: usize| {
            let regulatory_limit = if self.regulatory_override {
                self.maximum
            } else {
                84
            };
            self.target[target_index]
                .min(regulatory_limit)
                .min(self.maximum)
        };
        let primary = target(index);
        let alternate = if (6..=9).contains(&index) {
            target(index + 8)
        } else {
            primary
        };
        PhyTxTargetPowerPair {
            primary: primary >> 2,
            alternate: alternate >> 2,
        }
    }
}

/// Exact valid-index portion of ROM `phy_rate_to_index` at 0x2f82_491e.
const fn rate_to_target_index(rate: u8) -> Option<usize> {
    let index = match rate {
        0..=7 => ((rate >> 1) & 1) as usize,
        8 => 5,
        9 => 4,
        10 => 3,
        11 => 2,
        12 => 5,
        13 => 4,
        14 => 3,
        15 => 2,
        16..=23 => ((((rate as i8) >> 1) as u8 & 7) + 6) as usize,
        24..=31 => (rate - 14) as usize,
        41 | 42 => 0,
        _ => return None,
    };
    Some(index)
}

const TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;
const CHANNEL_CODES: [u16; 3] = [1, 6, 11];
const POWER_CONTROL_MAX_ITERATIONS: u8 = 10;

const fn average_measured_power(first: i16, second: i16) -> i16 {
    let average = first.wrapping_add(second).wrapping_add(4) >> 3;
    if average < 0 { 0 } else { average }
}

const fn tx_cap_value(capacitance: [u8; 6], channel_code: u16) -> u8 {
    let index = if channel_code <= 3 {
        0
    } else if channel_code <= 8 {
        2
    } else {
        4
    };
    capacitance[index] | 0xc0
}

/// Direct channel-six TX-cap publication used by vendor
/// `phy_bt_tx_gain_init` before its Bluetooth calibration children.
pub(crate) const fn bluetooth_tx_cap_action(capacitance: [u8; 6]) -> PhyTxPowerAction {
    PhyTxPowerAction::WriteI2c {
        address: TX_CAP_ADDRESS,
        value: tx_cap_value(capacitance, 6),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointRequest {
    pub identity: u8,
    pub target: i16,
    pub tone_selector: u16,
    pub base_attenuation: i16,
    pub initial_serial_error: i16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointOutcome {
    pub identity: u8,
    pub correction: i8,
    pub attenuation: u8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointAction {
    ConfigureTone {
        identity: u8,
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarAction),
    StopTone {
        identity: u8,
    },
    Complete(PhyPowerControlPointOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::enum_variant_names,
    reason = "each completion names the tone action it answers: ConfigureTone, ToneSar, StopTone"
)]
pub enum PhyPowerControlPointCompletion {
    ToneConfigured {
        identity: u8,
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarCompletion),
    ToneStopped {
        identity: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PointStep {
    ConfigureTone,
    Measure {
        phase: u8,
        transition: PhyToneSarTransition,
    },
    StopTone,
    Complete,
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointTransition {
    request: PhyPowerControlPointRequest,
    step: PointStep,
    iteration: u8,
    serial_error: i16,
    previous_error: i16,
    attenuation: u8,
    first_power: i16,
}

impl PhyPowerControlPointTransition {
    pub const fn new(request: PhyPowerControlPointRequest) -> Self {
        Self {
            request,
            step: PointStep::ConfigureTone,
            iteration: 0,
            serial_error: request.initial_serial_error,
            previous_error: 2,
            attenuation: 0,
            first_power: 0,
        }
    }

    const fn requested_attenuation(&self) -> u8 {
        crate::calibration::math::saturate_signed(
            self.request
                .base_attenuation
                .wrapping_add(self.serial_error) as i32,
            100,
            0,
        ) as u8
    }

    const fn measurement_identity(&self, phase: u8) -> u8 {
        self.request
            .identity
            .wrapping_mul(32)
            .wrapping_add(self.iteration.wrapping_mul(2))
            .wrapping_add(phase)
    }

    fn measurement(&self, phase: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: self.measurement_identity(phase),
            samples: 2,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
        .expect("two samples are nonzero")
    }

    const fn outcome(&self) -> PhyPowerControlPointOutcome {
        // Complete ROM `phy_rfcal_pwrctrl` saturates only values below -24.
        // Wi-Fi's baseline 52 naturally caps the positive side at 48, while
        // Bluetooth's baseline 8 legitimately reaches 92.
        let raw_correction = self.attenuation as i16 - self.request.base_attenuation;
        let correction = if raw_correction < -24 {
            -24
        } else {
            raw_correction
        } as i8;
        PhyPowerControlPointOutcome {
            identity: self.request.identity,
            correction,
            attenuation: self.attenuation,
            iterations: self.iteration + 1,
        }
    }

    pub const fn action(&self) -> PhyPowerControlPointAction {
        match self.step {
            PointStep::ConfigureTone => PhyPowerControlPointAction::ConfigureTone {
                identity: self.request.identity,
                iteration: self.iteration,
                selector: self.request.tone_selector,
                attenuation: self.requested_attenuation(),
            },
            PointStep::Measure { transition, .. } => {
                PhyPowerControlPointAction::ToneSar(transition.action())
            }
            PointStep::StopTone => PhyPowerControlPointAction::StopTone {
                identity: self.request.identity,
            },
            PointStep::Complete => PhyPowerControlPointAction::Complete(self.outcome()),
            PointStep::Failed(failure) => PhyPowerControlPointAction::Failed(failure),
        }
    }

    fn finish_measurement(&mut self, second_power: i16) {
        let measured = average_measured_power(self.first_power, second_power);
        let error = crate::calibration::math::saturate_signed(
            measured.wrapping_sub(self.request.target) as i32,
            24,
            -24,
        ) as i16;
        let requested = self.requested_attenuation();
        self.attenuation = requested;

        let converged =
            (error == 0 && self.iteration != 0) || (error == -1 && self.previous_error == 1);
        let hit_limit = (requested == 0 && error < 0) || (requested == 100 && error > 0);
        if converged || hit_limit || self.iteration + 1 == POWER_CONTROL_MAX_ITERATIONS {
            self.step = PointStep::StopTone;
            return;
        }

        let mut serial = self.serial_error.wrapping_add(error);
        if !(-2..=2).contains(&error) {
            serial = serial.wrapping_sub(error >> 2);
        }
        self.serial_error = serial;
        self.previous_error = error;
        self.iteration += 1;
        self.step = PointStep::ConfigureTone;
    }

    pub fn advance(
        &mut self,
        completion: PhyPowerControlPointCompletion,
    ) -> Result<(), PhyPowerControlPointTransitionError> {
        match (self.step, completion) {
            (
                PointStep::ConfigureTone,
                PhyPowerControlPointCompletion::ToneConfigured {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && selector == self.request.tone_selector
                && attenuation == self.requested_attenuation() =>
            {
                self.attenuation = attenuation;
                self.step = PointStep::Measure {
                    phase: 0,
                    transition: self.measurement(0),
                };
            }
            (
                PointStep::Measure {
                    phase,
                    mut transition,
                },
                PhyPowerControlPointCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyPowerControlPointTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        let power = phy_tx_power_db(
                            outcome.sample,
                            self.request.reference_codes,
                            self.request.power_offset,
                        );
                        if phase == 0 {
                            self.first_power = power;
                            self.step = PointStep::Measure {
                                phase: 1,
                                transition: self.measurement(1),
                            };
                        } else {
                            self.finish_measurement(power);
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = PointStep::Failed(failure);
                    }
                    _ => {
                        self.step = PointStep::Measure { phase, transition };
                    }
                }
            }
            (PointStep::StopTone, PhyPowerControlPointCompletion::ToneStopped { identity })
                if identity == self.request.identity =>
            {
                self.step = PointStep::Complete;
            }
            (PointStep::Complete | PointStep::Failed(_), _) => {
                return Err(PhyPowerControlPointTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPowerControlPointTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerParameters {
    pub already_calibrated: bool,
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub capacitance: [u8; 6],
    pub target_adjustment: u8,
    pub power_offset: i16,
    pub initial_attenuation: u8,
    pub clear_tone_after_ready: bool,
    /// Existing SAR reference codes. Wi-Fi replaces these during its
    /// reference phase; Bluetooth consumes the values established earlier in
    /// the shared cold-calibration graph.
    pub reference_codes: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerOutcome {
    pub reference_codes: [i16; 2],
    pub power_curve: [i8; 3],
    pub point_corrections: [i8; 3],
    pub power_adjustment: i8,
    pub final_attenuation: u8,
    pub current_channel: u16,
    pub calibration_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    Rfpll(RfpllFrequencyFailure),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerAction {
    Environment(PhyTxCalibrationEnvironmentAction),
    Rfpll(RfpllFrequencyAction),
    WriteI2c {
        address: PhyI2cAddress,
        value: u8,
    },
    ConfigureTone {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    WriteReferenceControl {
        value: u16,
    },
    ToneSar(PhyToneSarAction),
    Point(PhyPowerControlPointAction),
    Complete(PhyTxPowerOutcome),
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerCompletion {
    Environment(PhyTxCalibrationEnvironmentCompletion),
    Rfpll(RfpllFrequencyCompletion),
    I2cWritten {
        address: PhyI2cAddress,
        value: u8,
    },
    ToneConfigured {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    ReferenceControlWritten {
        value: u16,
    },
    ToneSar(PhyToneSarCompletion),
    Point(PhyPowerControlPointCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPowerTerminal {
    Complete,
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPowerStep {
    Enter(PhyTxCalibrationEnvironmentTransition),
    Rfpll(RfpllFrequencyTransition),
    TxCap,
    ReferenceTone,
    ReferenceControl {
        phase: u8,
    },
    ReferenceSample {
        phase: u8,
        transition: PhyToneSarTransition,
    },
    Point(PhyPowerControlPointTransition),
    Exit {
        terminal: TxPowerTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Complete,
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxPowerMode {
    Wifi,
    Bluetooth { tone_selector: u16 },
}

impl PhyTxPowerMode {
    const fn base_attenuation(self) -> i16 {
        match self {
            Self::Wifi => 52,
            Self::Bluetooth { .. } => 8,
        }
    }

    const fn tone_selector(self) -> u16 {
        match self {
            Self::Wifi => 0x80,
            Self::Bluetooth { tone_selector } => tone_selector,
        }
    }

    const fn adjustment_center(self) -> i16 {
        match self {
            Self::Wifi => 12,
            Self::Bluetooth { .. } => 10,
        }
    }

    const fn accepted_first_power(self) -> core::ops::RangeInclusive<i16> {
        match self {
            Self::Wifi => 0..=16,
            Self::Bluetooth { .. } => 0..=22,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTransition {
    parameters: PhyTxPowerParameters,
    mode: PhyTxPowerMode,
    step: TxPowerStep,
    channel: u8,
    reference_codes: [i16; 2],
    power_curve: [i8; 3],
    point_corrections: [i8; 3],
    power_adjustment: i8,
    attenuation: u8,
}

impl PhyTxPowerTransition {
    pub const fn new(parameters: PhyTxPowerParameters) -> Self {
        Self::new_for_mode(parameters, PhyTxPowerMode::Wifi)
    }

    /// Construct the mode-one branch of archive
    /// `phy_tx_pwctrl_init_cal_new`, used by `phy_bt_tx_pwctrl_init`.
    pub const fn new_bluetooth(parameters: PhyTxPowerParameters, tone_selector: u16) -> Self {
        Self::new_for_mode(parameters, PhyTxPowerMode::Bluetooth { tone_selector })
    }

    const fn new_for_mode(parameters: PhyTxPowerParameters, mode: PhyTxPowerMode) -> Self {
        let step = if parameters.already_calibrated {
            TxPowerStep::Complete
        } else {
            match mode {
                PhyTxPowerMode::Wifi => TxPowerStep::Enter(
                    PhyTxCalibrationEnvironmentTransition::enter(parameters.environment),
                ),
                // The Bluetooth parent owns debug-mode entry. Its shared
                // child first applies the channel-six TX-cap row, then begins
                // the three-channel RFPLL search.
                PhyTxPowerMode::Bluetooth { .. } => TxPowerStep::TxCap,
            }
        };
        Self {
            parameters,
            mode,
            step,
            channel: 0,
            reference_codes: parameters.reference_codes,
            power_curve: [0; 3],
            point_corrections: [0; 3],
            power_adjustment: 0,
            attenuation: parameters.initial_attenuation,
        }
    }

    const fn outcome(&self) -> PhyTxPowerOutcome {
        PhyTxPowerOutcome {
            reference_codes: self.reference_codes,
            power_curve: self.power_curve,
            point_corrections: self.point_corrections,
            power_adjustment: self.power_adjustment,
            final_attenuation: self.attenuation,
            // The Wi-Fi calibration publishes its final channel into the
            // shared PHY state. The vendor Bluetooth wrapper reuses the
            // three-channel calibration search without publishing that
            // implementation detail into the same field.
            current_channel: match (self.parameters.already_calibrated, self.mode) {
                (true, _) | (false, PhyTxPowerMode::Bluetooth { .. }) => 0,
                (false, PhyTxPowerMode::Wifi) => 11,
            },
            calibration_performed: !self.parameters.already_calibrated,
        }
    }

    fn exit(&mut self, terminal: TxPowerTerminal) {
        self.step = match self.mode {
            PhyTxPowerMode::Wifi => TxPowerStep::Exit {
                terminal,
                transition: PhyTxCalibrationEnvironmentTransition::exit(
                    self.parameters.environment,
                ),
            },
            PhyTxPowerMode::Bluetooth { .. } => match terminal {
                TxPowerTerminal::Complete => TxPowerStep::Complete,
                TxPowerTerminal::Failed(failure) => TxPowerStep::Failed(failure),
            },
        };
    }

    fn fail(&mut self, failure: PhyTxPowerFailure) {
        self.exit(TxPowerTerminal::Failed(failure));
    }

    fn rfpll(&self) -> RfpllFrequencyTransition {
        RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
            crystal_selector: self.parameters.crystal_selector,
            frequency_code: CHANNEL_CODES[self.channel as usize],
            offset: 0,
        })
    }

    const fn tx_cap_channel_code(&self) -> u16 {
        match self.mode {
            PhyTxPowerMode::Wifi => CHANNEL_CODES[self.channel as usize],
            PhyTxPowerMode::Bluetooth { .. } => 6,
        }
    }

    fn reference_sample(&self, phase: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: 0x60 + phase,
            samples: 4,
            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
        })
        .expect("four samples are nonzero")
    }

    fn point(&self) -> PhyPowerControlPointTransition {
        let base_attenuation = self.mode.base_attenuation();
        PhyPowerControlPointTransition::new(PhyPowerControlPointRequest {
            identity: self.channel,
            target: 56_i16.wrapping_sub(self.parameters.target_adjustment as i16),
            tone_selector: self.mode.tone_selector(),
            base_attenuation,
            initial_serial_error: (self.attenuation as i16).wrapping_sub(base_attenuation),
            power_offset: self.parameters.power_offset,
            reference_codes: self.reference_codes,
            // `phy_tx_pwctrl_init_cal_new` forces former
            // `phy_param[0x1aa] = 1` around all three point searches.
            clear_tone_after_ready: true,
        })
    }

    fn finish_points(&mut self) {
        let first = self.power_curve[0] as i16;
        let adjustment = if !self.mode.accepted_first_power().contains(&first) {
            crate::calibration::math::saturate_signed(
                (first - self.mode.adjustment_center()) as i32,
                40,
                -40,
            ) as i8
        } else {
            0
        };
        self.power_adjustment = (56_i16
            .wrapping_sub(self.parameters.target_adjustment as i16)
            .wrapping_add(adjustment as i16)) as i8;
        if adjustment != 0 {
            let mut index = 0;
            while index != self.power_curve.len() {
                self.power_curve[index] = self.power_curve[index].wrapping_sub(adjustment);
                index += 1;
            }
        }
        self.exit(TxPowerTerminal::Complete);
    }

    pub const fn action(&self) -> PhyTxPowerAction {
        match self.step {
            TxPowerStep::Enter(transition) | TxPowerStep::Exit { transition, .. } => {
                PhyTxPowerAction::Environment(transition.action())
            }
            TxPowerStep::Rfpll(transition) => PhyTxPowerAction::Rfpll(transition.action()),
            TxPowerStep::TxCap => PhyTxPowerAction::WriteI2c {
                address: TX_CAP_ADDRESS,
                value: tx_cap_value(self.parameters.capacitance, self.tx_cap_channel_code()),
            },
            TxPowerStep::ReferenceTone => PhyTxPowerAction::ConfigureTone {
                selector: 0x80,
                attenuation: 0x50,
                enabled: true,
            },
            TxPowerStep::ReferenceControl { phase } => PhyTxPowerAction::WriteReferenceControl {
                value: match phase {
                    0 => 0,
                    1 => 0x5555,
                    _ => 0xaaaa,
                },
            },
            TxPowerStep::ReferenceSample { transition, .. } => {
                PhyTxPowerAction::ToneSar(transition.action())
            }
            TxPowerStep::Point(transition) => PhyTxPowerAction::Point(transition.action()),
            TxPowerStep::Complete => PhyTxPowerAction::Complete(self.outcome()),
            TxPowerStep::Failed(failure) => PhyTxPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxPowerCompletion,
    ) -> Result<(), PhyTxPowerTransitionError> {
        match (self.step, completion) {
            (TxPowerStep::Enter(mut transition), PhyTxPowerCompletion::Environment(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => self.step = TxPowerStep::Rfpll(self.rfpll()),
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::Environment(failure));
                    }
                    _ => self.step = TxPowerStep::Enter(transition),
                }
            }
            (TxPowerStep::Rfpll(mut transition), PhyTxPowerCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => {
                        self.step = match self.mode {
                            PhyTxPowerMode::Wifi => TxPowerStep::TxCap,
                            PhyTxPowerMode::Bluetooth { .. } => TxPowerStep::Point(self.point()),
                        }
                    }
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::Rfpll(failure));
                    }
                    _ => self.step = TxPowerStep::Rfpll(transition),
                }
            }
            (TxPowerStep::TxCap, PhyTxPowerCompletion::I2cWritten { address, value })
                if address == TX_CAP_ADDRESS
                    && value
                        == tx_cap_value(
                            self.parameters.capacitance,
                            self.tx_cap_channel_code(),
                        ) =>
            {
                self.step = match self.mode {
                    PhyTxPowerMode::Wifi if self.channel == 0 => TxPowerStep::ReferenceTone,
                    PhyTxPowerMode::Wifi => TxPowerStep::Point(self.point()),
                    PhyTxPowerMode::Bluetooth { .. } => TxPowerStep::Rfpll(self.rfpll()),
                };
            }
            (
                TxPowerStep::ReferenceTone,
                PhyTxPowerCompletion::ToneConfigured {
                    selector: 0x80,
                    attenuation: 0x50,
                    enabled: true,
                },
            ) => self.step = TxPowerStep::ReferenceControl { phase: 0 },
            (
                TxPowerStep::ReferenceControl { phase },
                PhyTxPowerCompletion::ReferenceControlWritten { value },
            ) if value
                == match phase {
                    0 => 0,
                    1 => 0x5555,
                    _ => 0xaaaa,
                } =>
            {
                if phase == 2 {
                    self.step = TxPowerStep::Point(self.point());
                } else {
                    self.step = TxPowerStep::ReferenceSample {
                        phase,
                        transition: self.reference_sample(phase),
                    };
                }
            }
            (
                TxPowerStep::ReferenceSample {
                    phase,
                    mut transition,
                },
                PhyTxPowerCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        self.reference_codes[phase as usize] = outcome.sample as i16;
                        self.step = TxPowerStep::ReferenceControl { phase: phase + 1 };
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::ToneSar(failure));
                    }
                    _ => {
                        self.step = TxPowerStep::ReferenceSample { phase, transition };
                    }
                }
            }
            (TxPowerStep::Point(mut transition), PhyTxPowerCompletion::Point(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerControlPointAction::Complete(outcome) => {
                        let index = self.channel as usize;
                        self.point_corrections[index] = outcome.correction;
                        self.power_curve[index] = if self.channel == 1 {
                            outcome.correction.wrapping_add(2)
                        } else {
                            outcome.correction
                        };
                        self.attenuation = self.power_curve[index]
                            .wrapping_add(self.mode.base_attenuation() as i8)
                            as u8;
                        self.channel += 1;
                        if self.channel == 3 {
                            self.finish_points();
                        } else {
                            self.step = TxPowerStep::Rfpll(self.rfpll());
                        }
                    }
                    PhyPowerControlPointAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::ToneSar(failure));
                    }
                    _ => self.step = TxPowerStep::Point(transition),
                }
            }
            (
                TxPowerStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxPowerCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => {
                        self.step = match terminal {
                            TxPowerTerminal::Complete => TxPowerStep::Complete,
                            TxPowerTerminal::Failed(failure) => TxPowerStep::Failed(failure),
                        };
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.step = TxPowerStep::Failed(PhyTxPowerFailure::Environment(failure));
                    }
                    _ => {
                        self.step = TxPowerStep::Exit {
                            terminal,
                            transition,
                        };
                    }
                }
            }
            (TxPowerStep::Complete | TxPowerStep::Failed(_), _) => {
                return Err(PhyTxPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxPowerTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointMmioBinding {
    action: PhyPowerControlPointAction,
}

impl PhyPowerControlPointMmioBinding {
    pub fn new(action: PhyPowerControlPointAction) -> Result<Self, PhyTxPowerBindingError> {
        match action {
            PhyPowerControlPointAction::ConfigureTone { .. }
            | PhyPowerControlPointAction::StopTone { .. } => Ok(Self { action }),
            _ => Err(PhyTxPowerBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyPowerControlPointCompletion {
        match self.action {
            PhyPowerControlPointAction::ConfigureTone {
                identity,
                iteration,
                selector,
                attenuation,
            } => {
                crate::hardware::configure_phy_power_control_tone(registers, selector, attenuation);
                PhyPowerControlPointCompletion::ToneConfigured {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                }
            }
            PhyPowerControlPointAction::StopTone { identity } => {
                crate::hardware::stop_phy_power_detector_tone(registers);
                PhyPowerControlPointCompletion::ToneStopped { identity }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerMmioBinding {
    action: PhyTxPowerAction,
}

impl PhyTxPowerMmioBinding {
    pub fn new(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerBindingError> {
        match action {
            PhyTxPowerAction::ConfigureTone { enabled: true, .. }
            | PhyTxPowerAction::WriteReferenceControl { .. } => Ok(Self { action }),
            _ => Err(PhyTxPowerBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxPowerCompletion {
        match self.action {
            PhyTxPowerAction::ConfigureTone {
                selector,
                attenuation,
                enabled: true,
            } => {
                crate::hardware::configure_phy_power_control_tone(registers, selector, attenuation);
                PhyTxPowerCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: true,
                }
            }
            PhyTxPowerAction::WriteReferenceControl { value } => {
                oer_esp32s31_hal::phy::power_detector::write_reference(registers, value);
                PhyTxPowerCompletion::ReferenceControlWritten { value }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointExternalBinding {
    Mmio(PhyPowerControlPointMmioBinding),
    ToneSar(crate::tx::calibration::PhyToneSarExternalBinding),
}

impl PhyPowerControlPointExternalBinding {
    pub fn lower(
        action: PhyPowerControlPointAction,
    ) -> Result<Self, PhyTxPowerExternalBindingError> {
        match action {
            PhyPowerControlPointAction::ConfigureTone { .. }
            | PhyPowerControlPointAction::StopTone { .. } => {
                PhyPowerControlPointMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyPowerControlPointAction::ToneSar(action) => {
                crate::tx::calibration::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyPowerControlPointAction::Complete(_) | PhyPowerControlPointAction::Failed(_) => {
                Err(PhyTxPowerExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerI2cBinding {
    address: PhyI2cAddress,
    value: u8,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyTxPowerI2cBinding {
    pub fn new(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerExternalBindingError> {
        let PhyTxPowerAction::WriteI2c { address, value } = action else {
            return Err(PhyTxPowerExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            value,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value),
            ),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(test)]
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    #[cfg(test)]
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

    pub fn into_completion(self) -> Result<PhyTxPowerCompletion, PhyTxPowerExternalBindingError> {
        match self.transaction.action() {
            crate::calibration::cold::PhyColdI2cAction::Complete(
                crate::calibration::cold::PhyColdI2cOutcome::Written { address },
            ) if address == self.address => Ok(PhyTxPowerCompletion::I2cWritten {
                address: self.address,
                value: self.value,
            }),
            crate::calibration::cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyTxPowerExternalBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyTxPowerExternalBindingError::IncompleteTransaction),
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_tx_pwctrl_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxPowerExternalBinding {
    Environment(crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding),
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyTxPowerI2cBinding),
    Mmio(PhyTxPowerMmioBinding),
    ToneSar(crate::tx::calibration::PhyToneSarExternalBinding),
    Point(PhyPowerControlPointExternalBinding),
}

impl PhyTxPowerExternalBinding {
    pub fn lower(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerExternalBindingError> {
        match action {
            PhyTxPowerAction::Environment(action) => {
                crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Environment)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::WriteI2c { .. } => PhyTxPowerI2cBinding::new(action).map(Self::I2c),
            PhyTxPowerAction::ConfigureTone { .. }
            | PhyTxPowerAction::WriteReferenceControl { .. } => PhyTxPowerMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction),
            PhyTxPowerAction::ToneSar(action) => {
                crate::tx::calibration::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::Point(action) => {
                PhyPowerControlPointExternalBinding::lower(action).map(Self::Point)
            }
            PhyTxPowerAction::Complete(_) | PhyTxPowerAction::Failed(_) => {
                Err(PhyTxPowerExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests;
