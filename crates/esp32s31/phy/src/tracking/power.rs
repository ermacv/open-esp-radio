//! Source-owned ESP32-S31 temperature-driven TX-power tracking.
//!
//! The pinned archive parent `phy_txpwr_cal_track_new` is 104 instructions in
//! 21 finite basic blocks. This module preserves its decision arithmetic,
//! state-commit point, BBPLL boundary and protocol-specific gain publication
//! order. The vendor's optional `phy_printf` call is diagnostic-only and has
//! no radio or retained-state effect, so it is deliberately absent.

use crate::{
    analog::pbus::{PhyForceTxRxAction, PhyForceTxRxCompletion, PhyForceTxRxTransition},
    calibration::math::{absolute_temperature, saturate_signed},
    tracking::parameters::{PhyCalibrationTrackClass, temperature_to_tracking_power},
};

/// Caller-owned inputs formerly read from the shared `phy_param` image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingParameters {
    pub current_temperature: i16,
    pub reference_temperature: i16,
    pub previous_tracking_temperature: i16,
    pub previous_tracking_gain_base: i8,
    pub wifi_gain_base: i8,
    pub bluetooth_ieee802154_gain_base: i8,
    pub relaxed_threshold: bool,
    /// Vendor `tx_gain_skip_publication`: the Wi-Fi gain child returns before
    /// its force-TX/RX pair and gain-memory publication.
    pub wifi_gain_publication_skipped: bool,
}

/// One invocation of the shared Wi-Fi or Bluetooth/IEEE 802.15.4 parent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingRequest {
    pub class: PhyCalibrationTrackClass,
    pub enabled: bool,
    /// Passed only to the Wi-Fi gain publisher. The Bluetooth/IEEE child uses
    /// its fixed gain-memory bank and ignores this value.
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
    let upper = match request.class {
        PhyCalibrationTrackClass::Wifi => 80,
        PhyCalibrationTrackClass::BluetoothIeee802154 => 105,
    };
    let bounded_temperature =
        saturate_signed(parameters.current_temperature as i32, upper, -60) as i16;
    let tracking_delta =
        (bounded_temperature as i32).wrapping_sub(parameters.previous_tracking_temperature as i32);
    let recomputed = absolute_temperature(tracking_delta) >= threshold as u32;
    let gain_base = if recomputed {
        temperature_to_tracking_power(
            bounded_temperature,
            parameters.reference_temperature,
            request.class,
        )
    } else {
        parameters.previous_tracking_gain_base
    };
    let published_gain_base = match request.class {
        PhyCalibrationTrackClass::Wifi => parameters.wifi_gain_base,
        PhyCalibrationTrackClass::BluetoothIeee802154 => parameters.bluetooth_ieee802154_gain_base,
    };

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
    pub class: PhyCalibrationTrackClass,
    pub gain_updated: bool,
    pub tracking_temperature: i16,
    pub tracking_gain_base: i8,
    pub wifi_gain_base: i8,
    pub bluetooth_ieee802154_gain_base: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingAction {
    SetBbpllCalibration {
        enabled: bool,
    },
    /// One edge of the gain child's outermost force-TX/RX pair.
    ForceTxRx(PhyForceTxRxAction),
    RegenerateWifiGain {
        channel: u16,
        gain_base: i8,
    },
    RegenerateBluetoothIeee802154Gain {
        gain_base: i8,
    },
    Complete(PhyTxPowerTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingCompletion {
    BbpllCalibrationSet { enabled: bool },
    ForceTxRx(PhyForceTxRxCompletion),
    WifiGainRegenerated { channel: u16, gain_base: i8 },
    BluetoothIeee802154GainRegenerated { gain_base: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingStep {
    BbpllOn,
    ForceTxRx(PhyForceTxRxTransition),
    RegenerateGain,
    ReleaseTxRx(PhyForceTxRxTransition),
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
            class: request.class,
            gain_updated: decision.update_required,
            tracking_temperature: parameters.previous_tracking_temperature,
            tracking_gain_base: parameters.previous_tracking_gain_base,
            wifi_gain_base: parameters.wifi_gain_base,
            bluetooth_ieee802154_gain_base: parameters.bluetooth_ieee802154_gain_base,
        };
        let step = if decision.update_required {
            outcome.tracking_temperature = parameters.current_temperature;
            outcome.tracking_gain_base = decision.gain_base;
            match request.class {
                PhyCalibrationTrackClass::Wifi => {
                    outcome.wifi_gain_base = decision.gain_base;
                }
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    outcome.bluetooth_ieee802154_gain_base = decision.gain_base;
                }
            }
            PhyTxPowerTrackingStep::BbpllOn
        } else {
            PhyTxPowerTrackingStep::Complete
        };

        Self {
            request,
            publishes: !(matches!(request.class, PhyCalibrationTrackClass::Wifi)
                && parameters.wifi_gain_publication_skipped),
            decision,
            outcome,
            step,
        }
    }

    #[cfg(test)]
    pub const fn decision(self) -> PhyTxPowerTrackingDecision {
        self.decision
    }

    pub const fn action(self) -> PhyTxPowerTrackingAction {
        match self.step {
            PhyTxPowerTrackingStep::BbpllOn => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
            }
            PhyTxPowerTrackingStep::ForceTxRx(child)
            | PhyTxPowerTrackingStep::ReleaseTxRx(child) => {
                PhyTxPowerTrackingAction::ForceTxRx(child.action())
            }
            PhyTxPowerTrackingStep::RegenerateGain => match self.request.class {
                PhyCalibrationTrackClass::Wifi => PhyTxPowerTrackingAction::RegenerateWifiGain {
                    channel: self.request.wifi_channel,
                    gain_base: self.decision.gain_base,
                },
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain {
                        gain_base: self.decision.gain_base,
                    }
                }
            },
            PhyTxPowerTrackingStep::BbpllOff => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: false }
            }
            PhyTxPowerTrackingStep::Complete => PhyTxPowerTrackingAction::Complete(self.outcome),
        }
    }

    const fn after_gain(&self) -> PhyTxPowerTrackingStep {
        if self.publishes {
            PhyTxPowerTrackingStep::ReleaseTxRx(PhyForceTxRxTransition::new(false))
        } else {
            PhyTxPowerTrackingStep::BbpllOff
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxPowerTrackingCompletion,
    ) -> Result<(), PhyTxPowerTrackingTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTxPowerTrackingStep::BbpllOn,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true },
            ) => {
                if self.publishes {
                    PhyTxPowerTrackingStep::ForceTxRx(PhyForceTxRxTransition::new(true))
                } else {
                    PhyTxPowerTrackingStep::RegenerateGain
                }
            }
            (
                PhyTxPowerTrackingStep::ForceTxRx(mut child),
                PhyTxPowerTrackingCompletion::ForceTxRx(completion),
            ) => {
                child
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTrackingTransitionError::WrongCompletion)?;
                if matches!(child.action(), PhyForceTxRxAction::Complete { .. }) {
                    PhyTxPowerTrackingStep::RegenerateGain
                } else {
                    PhyTxPowerTrackingStep::ForceTxRx(child)
                }
            }
            (
                PhyTxPowerTrackingStep::ReleaseTxRx(mut child),
                PhyTxPowerTrackingCompletion::ForceTxRx(completion),
            ) => {
                child
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTrackingTransitionError::WrongCompletion)?;
                if matches!(child.action(), PhyForceTxRxAction::Complete { .. }) {
                    PhyTxPowerTrackingStep::BbpllOff
                } else {
                    PhyTxPowerTrackingStep::ReleaseTxRx(child)
                }
            }
            (
                PhyTxPowerTrackingStep::RegenerateGain,
                PhyTxPowerTrackingCompletion::WifiGainRegenerated { channel, gain_base },
            ) if self.request.class == PhyCalibrationTrackClass::Wifi
                && channel == self.request.wifi_channel
                && gain_base == self.decision.gain_base =>
            {
                self.after_gain()
            }
            (
                PhyTxPowerTrackingStep::RegenerateGain,
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base },
            ) if self.request.class == PhyCalibrationTrackClass::BluetoothIeee802154
                && gain_base == self.decision.gain_base =>
            {
                self.after_gain()
            }
            (
                PhyTxPowerTrackingStep::BbpllOff,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: false },
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

#[derive(Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingExternalOperation {
    SetBbpllCalibration {
        enabled: bool,
    },
    ForceTxRx(PhyForceTxRxAction),
    RegenerateWifiGain {
        channel: u16,
        gain_base: i8,
        image: Option<crate::channel::PhyWifiTxGainImage>,
    },
    RegenerateBluetoothIeee802154Gain {
        gain_base: i8,
        image: crate::calibration::bluetooth::PhyBluetoothTxGainImage,
    },
}

/// Non-cloneable owner of one external power-tracking edge.
///
/// Lowering captures a gain image from the live typed state before MMIO. This
/// prevents a later state change from separating the completion identity from
/// the bytes actually published. A missing Wi-Fi image is the reviewed vendor
/// `tx_gain_skip_publication` path and still completes the regeneration child.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingExternalBinding {
    operation: PhyTxPowerTrackingExternalOperation,
}

impl PhyTxPowerTrackingExternalBinding {
    pub fn lower(
        action: PhyTxPowerTrackingAction,
        state: &crate::state::PhyState,
    ) -> Result<Self, PhyTxPowerTrackingBindingError> {
        let operation = match action {
            PhyTxPowerTrackingAction::SetBbpllCalibration { enabled } => {
                PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled }
            }
            PhyTxPowerTrackingAction::ForceTxRx(
                action @ (PhyForceTxRxAction::Configure { .. }
                | PhyForceTxRxAction::DelayMicros { .. }),
            ) => PhyTxPowerTrackingExternalOperation::ForceTxRx(action),
            PhyTxPowerTrackingAction::ForceTxRx(PhyForceTxRxAction::Complete { .. }) => {
                return Err(PhyTxPowerTrackingBindingError::UnsupportedAction);
            }
            PhyTxPowerTrackingAction::RegenerateWifiGain { channel, gain_base } => {
                PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                    channel,
                    gain_base,
                    image: state.wifi_tracking_gain_image(channel, gain_base),
                }
            }
            PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base } => {
                PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                    gain_base,
                    image: state.bluetooth_ieee802154_tracking_gain_image(gain_base),
                }
            }
            PhyTxPowerTrackingAction::Complete(_) => {
                return Err(PhyTxPowerTrackingBindingError::UnsupportedAction);
            }
        };
        Ok(Self { operation })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyTxPowerTrackingAction {
        match self.operation {
            PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled } => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled }
            }
            PhyTxPowerTrackingExternalOperation::ForceTxRx(action) => {
                PhyTxPowerTrackingAction::ForceTxRx(action)
            }
            PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                channel, gain_base, ..
            } => PhyTxPowerTrackingAction::RegenerateWifiGain { channel, gain_base },
            PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                gain_base,
                ..
            } => PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base },
        }
    }

    /// The settle a force-TX/RX timer edge requests; the caller's short-delay
    /// backend must elapse it before [`Self::execute_target`] completes it.
    pub const fn delay_micros(&self) -> Option<u32> {
        match self.operation {
            PhyTxPowerTrackingExternalOperation::ForceTxRx(PhyForceTxRxAction::DelayMicros {
                micros,
                ..
            }) => Some(micros),
            _ => None,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P>(
        self,
        _platform: &mut P,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxPowerTrackingCompletion {
        match self.operation {
            PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled } => {
                oer_esp32s31_hal::phy::i2c::configure_bbpll_calibration(registers, enabled);
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled }
            }
            PhyTxPowerTrackingExternalOperation::ForceTxRx(PhyForceTxRxAction::Configure {
                enabled,
                phase,
            }) => {
                oer_esp32s31_hal::phy::pbus::configure_force_txrx(registers, enabled, phase);
                PhyTxPowerTrackingCompletion::ForceTxRx(PhyForceTxRxCompletion::Configured {
                    enabled,
                    phase,
                })
            }
            PhyTxPowerTrackingExternalOperation::ForceTxRx(PhyForceTxRxAction::DelayMicros {
                enabled,
                completed_phase,
                micros,
            }) => PhyTxPowerTrackingCompletion::ForceTxRx(PhyForceTxRxCompletion::DelayElapsed {
                enabled,
                completed_phase,
                micros,
            }),
            PhyTxPowerTrackingExternalOperation::ForceTxRx(PhyForceTxRxAction::Complete {
                ..
            }) => unreachable!(),
            PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                channel,
                gain_base,
                image,
            } => {
                if let Some(image) = image {
                    crate::hardware::publish_phy_tx_gain_memory(registers, false, image);
                }
                PhyTxPowerTrackingCompletion::WifiGainRegenerated { channel, gain_base }
            }
            PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                gain_base,
                image,
            } => {
                crate::hardware::publish_bluetooth_tx_gain_memory(registers, image);
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base }
            }
        }
    }
}

#[cfg(test)]
mod tests;
