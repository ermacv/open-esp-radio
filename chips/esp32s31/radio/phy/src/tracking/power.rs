//! Source-owned ESP32-S31 temperature-driven TX-power tracking.
//!
//! The pinned archive parent `phy_txpwr_cal_track_new` is 104 instructions in
//! 21 finite basic blocks. This module preserves its decision arithmetic,
//! state-commit point, BBPLL boundary and Wi-Fi's gain publication order. The vendor's optional `phy_printf` call is diagnostic-only and has
//! no radio or retained-state effect, so it is deliberately absent.

use crate::{
    analog::pbus::{PhyForceTxRxCompletion, PhyForceTxRxEdge, PhyForceTxRxRunning},
    calibration::math::{absolute_temperature, saturate_signed},
    tracking::parameters::temperature_to_tracking_power,
};
use oer_chips_esp32s31_hal::types::PhyBbpllCalibration;

/// Caller-owned inputs that the vendor reads from the shared `phy_param` image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingParameters {
    pub current_temperature: i16,
    pub reference_temperature: i16,
    pub previous_tracking_temperature: i16,
    pub previous_tracking_gain_base: i8,
    pub wifi_gain_base: i8,
    pub relaxed_threshold: bool,
    /// Vendor `tx_gain_skip_publication`: the Wi-Fi gain child returns before
    /// its force-TX/RX pair and gain-memory publication.
    pub wifi_gain_publication_skipped: bool,
}

/// One invocation of Wi-Fi's transmit-power tracking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingRequest {
    pub enabled: bool,
    /// The channel the gain publisher regenerates for.
    pub wifi_channel: u16,
}

/// Pure decision result before any retained state or hardware is changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingDecision {
    pub bounded_temperature: i16,
    pub threshold: u8,
    pub gain_base: i8,
    pub recomputed: bool,
    pub update_required: bool,
}

/// Compute the exact branch decision of `phy_txpwr_cal_track_new`.
pub const fn decide_tx_power_tracking(
    request: PhyTxPowerTrackingRequest,
    parameters: PhyTxPowerTrackingParameters,
) -> PhyTxPowerTrackingDecision {
    let reference_delta = (parameters.current_temperature as i32)
        .wrapping_sub(parameters.reference_temperature as i32);
    let threshold = if parameters.relaxed_threshold {
        10
    } else if absolute_temperature(reference_delta) > 7 {
        4
    } else {
        2
    };
    // Wi-Fi's upper bound.
    let bounded_temperature =
        saturate_signed(parameters.current_temperature as i32, 80, -60) as i16;
    let tracking_delta =
        (bounded_temperature as i32).wrapping_sub(parameters.previous_tracking_temperature as i32);
    let recomputed = absolute_temperature(tracking_delta) >= threshold as u32;
    let gain_base = if recomputed {
        temperature_to_tracking_power(bounded_temperature, parameters.reference_temperature)
    } else {
        parameters.previous_tracking_gain_base
    };
    let published_gain_base = parameters.wifi_gain_base;

    PhyTxPowerTrackingDecision {
        bounded_temperature,
        threshold,
        gain_base,
        recomputed,
        update_required: request.enabled && gain_base != published_gain_base,
    }
}

/// Retained result committed only after the gain child and BBPLL tail finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingOutcome {
    pub gain_updated: bool,
    pub tracking_temperature: i16,
    pub tracking_gain_base: i8,
    pub wifi_gain_base: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingAction {
    SetBbpllCalibration {
        calibration: PhyBbpllCalibration,
    },
    /// One edge of the gain child's outermost force-TX/RX pair.
    ForceTxRx(PhyForceTxRxEdge),
    RegenerateWifiGain {
        channel: u16,
        gain_base: i8,
    },
    Complete(PhyTxPowerTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingCompletion {
    BbpllCalibrationSet { calibration: PhyBbpllCalibration },
    ForceTxRx(PhyForceTxRxCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingStep {
    BbpllOn,
    ForceTxRx(PhyForceTxRxRunning),
    RegenerateGain,
    ReleaseTxRx(PhyForceTxRxRunning),
    BbpllOff,
    Complete,
}

/// Exact finite I/O ordering of archive `phy_txpwr_cal_track_new`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingTransition {
    request: PhyTxPowerTrackingRequest,
    /// The gain child publishes and therefore brackets itself with the
    /// outermost `phy_force_txrx_off_new` pair; power tracking runs at
    /// nesting level zero.
    publishes: bool,
    decision: PhyTxPowerTrackingDecision,
    outcome: PhyTxPowerTrackingOutcome,
    step: PhyTxPowerTrackingStep,
}

impl PhyTxPowerTrackingTransition {
    pub const fn new(
        request: PhyTxPowerTrackingRequest,
        parameters: PhyTxPowerTrackingParameters,
    ) -> Self {
        let decision = decide_tx_power_tracking(request, parameters);
        let mut outcome = PhyTxPowerTrackingOutcome {
            gain_updated: decision.update_required,
            tracking_temperature: parameters.previous_tracking_temperature,
            tracking_gain_base: parameters.previous_tracking_gain_base,
            wifi_gain_base: parameters.wifi_gain_base,
        };
        let step = if decision.update_required {
            outcome.tracking_temperature = parameters.current_temperature;
            outcome.tracking_gain_base = decision.gain_base;
            outcome.wifi_gain_base = decision.gain_base;
            PhyTxPowerTrackingStep::BbpllOn
        } else {
            PhyTxPowerTrackingStep::Complete
        };

        Self {
            request,
            publishes: !parameters.wifi_gain_publication_skipped,
            decision,
            outcome,
            step,
        }
    }

    pub(crate) const fn request(&self) -> PhyTxPowerTrackingRequest {
        self.request
    }

    pub const fn action(self) -> PhyTxPowerTrackingAction {
        match self.step {
            PhyTxPowerTrackingStep::BbpllOn => PhyTxPowerTrackingAction::SetBbpllCalibration {
                calibration: PhyBbpllCalibration::Enabled,
            },
            PhyTxPowerTrackingStep::ForceTxRx(child)
            | PhyTxPowerTrackingStep::ReleaseTxRx(child) => {
                PhyTxPowerTrackingAction::ForceTxRx(child.edge())
            }
            PhyTxPowerTrackingStep::RegenerateGain => {
                PhyTxPowerTrackingAction::RegenerateWifiGain {
                    channel: self.request.wifi_channel,
                    gain_base: self.decision.gain_base,
                }
            }
            PhyTxPowerTrackingStep::BbpllOff => PhyTxPowerTrackingAction::SetBbpllCalibration {
                calibration: PhyBbpllCalibration::Disabled,
            },
            PhyTxPowerTrackingStep::Complete => PhyTxPowerTrackingAction::Complete(self.outcome),
        }
    }

    const fn after_gain(&self) -> PhyTxPowerTrackingStep {
        if self.publishes {
            PhyTxPowerTrackingStep::ReleaseTxRx(PhyForceTxRxRunning::start(false))
        } else {
            PhyTxPowerTrackingStep::BbpllOff
        }
    }

    /// Internal acknowledgement after the cursor consumed every complete record
    /// (or the unchanged skip branch). The same live child owns the request.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn gain_published(&mut self) -> Result<(), PhyTxPowerTrackingTransitionError> {
        if !matches!(self.step, PhyTxPowerTrackingStep::RegenerateGain) {
            return Err(PhyTxPowerTrackingTransitionError::WrongCompletion);
        }
        self.step = self.after_gain();
        Ok(())
    }

    #[inline(always)]
    pub fn advance(
        &mut self,
        completion: PhyTxPowerTrackingCompletion,
    ) -> Result<(), PhyTxPowerTrackingTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTxPowerTrackingStep::BbpllOn,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
                    calibration: PhyBbpllCalibration::Enabled,
                },
            ) => {
                if self.publishes {
                    PhyTxPowerTrackingStep::ForceTxRx(PhyForceTxRxRunning::start(true))
                } else {
                    PhyTxPowerTrackingStep::RegenerateGain
                }
            }
            (
                PhyTxPowerTrackingStep::ForceTxRx(child),
                PhyTxPowerTrackingCompletion::ForceTxRx(completion),
            ) => match child
                .advance(completion)
                .map_err(|_| PhyTxPowerTrackingTransitionError::WrongCompletion)?
            {
                Some(child) => PhyTxPowerTrackingStep::ForceTxRx(child),
                None => PhyTxPowerTrackingStep::RegenerateGain,
            },
            (
                PhyTxPowerTrackingStep::ReleaseTxRx(child),
                PhyTxPowerTrackingCompletion::ForceTxRx(completion),
            ) => match child
                .advance(completion)
                .map_err(|_| PhyTxPowerTrackingTransitionError::WrongCompletion)?
            {
                Some(child) => PhyTxPowerTrackingStep::ReleaseTxRx(child),
                None => PhyTxPowerTrackingStep::BbpllOff,
            },
            (
                PhyTxPowerTrackingStep::BbpllOff,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
                    calibration: PhyBbpllCalibration::Disabled,
                },
            ) => PhyTxPowerTrackingStep::Complete,
            (PhyTxPowerTrackingStep::Complete, _) => {
                return Err(PhyTxPowerTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxPowerTrackingTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingBindingError {
    UnsupportedAction,
}

/// Affine small control edge. A stepped gain publisher never lowers a whole
/// image through this route, even on the conservative common-driver path.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct ControlBinding(ControlEdge);
/// The control edges a [`ControlBinding`] admits.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy)]
enum ControlEdge {
    SetBbpllCalibration { calibration: PhyBbpllCalibration },
    ForceTxRx(PhyForceTxRxEdge),
}
#[cfg(any(target_arch = "riscv32", test))]
impl ControlBinding {
    pub fn lower(action: PhyTxPowerTrackingAction) -> Result<Self, PhyTxPowerTrackingBindingError> {
        match action {
            PhyTxPowerTrackingAction::SetBbpllCalibration { calibration } => {
                Ok(Self(ControlEdge::SetBbpllCalibration { calibration }))
            }
            PhyTxPowerTrackingAction::ForceTxRx(edge) => Ok(Self(ControlEdge::ForceTxRx(edge))),
            _ => Err(PhyTxPowerTrackingBindingError::UnsupportedAction),
        }
    }
    pub const fn delay_micros(&self) -> Option<u32> {
        match self.0 {
            ControlEdge::ForceTxRx(PhyForceTxRxEdge::DelayMicros { micros, .. }) => Some(micros),
            _ => None,
        }
    }
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_chips_esp32s31_hal::system::owner::SharedPhyAccess,
    ) -> PhyTxPowerTrackingCompletion {
        match self.0 {
            ControlEdge::SetBbpllCalibration { calibration } => {
                oer_chips_esp32s31_hal::radio::phy::i2c::configure_bbpll_calibration(
                    registers,
                    calibration,
                );
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { calibration }
            }
            ControlEdge::ForceTxRx(PhyForceTxRxEdge::Configure { phase }) => {
                oer_chips_esp32s31_hal::radio::phy::pbus::configure_force_txrx(registers, phase);
                PhyTxPowerTrackingCompletion::ForceTxRx(PhyForceTxRxCompletion::Configured {
                    phase,
                })
            }
            ControlEdge::ForceTxRx(PhyForceTxRxEdge::DelayMicros {
                enabled,
                completed_phase,
                micros,
            }) => PhyTxPowerTrackingCompletion::ForceTxRx(PhyForceTxRxCompletion::DelayElapsed {
                enabled,
                completed_phase,
                micros,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
