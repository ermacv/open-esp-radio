//! Owned outer transition for the current S31 `phy_param_track_tot` order.
//!
//! Reference: the pinned esp-phy-lib `libphy.a` of
//! `chips/esp32s31/vendor/artifacts.toml`.
//! RFPLL precedes BT/154 power, Wi-Fi I2C/power, one combined RX/TX
//! calibration child, then temperature acquisition. The finite graph is
//! separate from physical admission and child hardware equivalence. The
//! registered owner is retained across every child and poisoned on incomplete
//! hardware execution. Vendor grant hooks protect individual RFPLL, RX and TX
//! regions, and the RFPLL and calibration children emit them at the same
//! positions; OER also retains its wider exclusive maintenance boundary.

use core::fmt;

/// Active protocol classes passed by the shared-PHY scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackRequest {
    wifi: bool,
    bluetooth_ieee802154: bool,
}

impl PhyParamTrackRequest {
    pub const fn new(wifi: bool, bluetooth_ieee802154: bool) -> Self {
        Self {
            wifi,
            bluetooth_ieee802154,
        }
    }

    pub const fn wifi(self) -> bool {
        self.wifi
    }

    pub const fn bluetooth_ieee802154(self) -> bool {
        self.bluetooth_ieee802154
    }
}

/// Diagnostic-print policy forwarded to the recovered vendor children.
///
/// Diagnostics do not identify or acknowledge hardware work. They remain on
/// the action so a target observer can reproduce the source-visible policy,
/// but are deliberately absent from completion identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTrackingDiagnostics {
    Disabled,
    /// The vendor's diagnostic printing. The pinned ESP32-S31 policy
    /// ([`PhyParamTrackingPolicy::for_registered_state`]) never selects it;
    /// host tests drive the children with it to show it changes no hardware
    /// work.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the pinned ESP32-S31 policy disables diagnostic printing"
        )
    )]
    Enabled,
}

impl PhyTrackingDiagnostics {
    #[cfg(any(test, oer_probe))]
    pub const fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Driver-owned projections consumed by the outer tracking function.
///
/// `tracking_inhibited` is the OR of the two vendor guard bytes. Collapsing
/// them is exact because the body never distinguishes which guard stopped the
/// operation. Production code can obtain this policy only from a registered
/// PHY epoch; the type is crate-private so integration cannot manufacture a
/// second parameter-image model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyParamTrackingPolicy {
    pub(crate) tracking_inhibited: bool,
    pub(crate) rfpll_cap_tracking_enabled: bool,
    pub(crate) rfpll_cap_tracking_threshold: Option<u8>,
    pub(crate) calibration_tracking_threshold: Option<u8>,
    pub(crate) diagnostics: PhyTrackingDiagnostics,
    pub(crate) bluetooth_ieee802154_power_tracking_enabled: bool,
    pub(crate) calibration_tracking_enabled: bool,
    pub(crate) relaxed_power_tracking_threshold: bool,
}

impl PhyParamTrackingPolicy {
    /// Project the pinned ESP32-S31 policy for one active registered epoch.
    ///
    /// Both lifecycle guards are clear, diagnostic printing is disabled, and
    /// calibration and RFPLL-cap tracking are enabled. Compiled comparisons
    /// cover current child effects and the RFPLL-enabled parent. The selected
    /// RFPLL action already uses `tracking::rfpll::thermal`, including the
    /// current measured search and frequency-control envelope. Runtime service
    /// selection separately requires a fresh dated temperature observation.
    /// Runtime setters publish the remaining choices into `PhyState`.
    pub(crate) const fn for_registered_state(state: &crate::state::PhyState) -> Self {
        let debug = state.temperature_tracking_debug();
        Self {
            tracking_inhibited: false,
            rfpll_cap_tracking_enabled: true,
            rfpll_cap_tracking_threshold: debug.rfpll_threshold_override(),
            calibration_tracking_threshold: debug.calibration_threshold_override(),
            diagnostics: PhyTrackingDiagnostics::Disabled,
            bluetooth_ieee802154_power_tracking_enabled: state.bt_power_tracking() != 0,
            calibration_tracking_enabled: true,
            relaxed_power_tracking_threshold: state.tx_power_tracking_slow() != 0,
        }
    }
}

/// TX calibration/gain class inside the shared RX/TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackClass {
    Wifi,
    BluetoothIeee802154,
}

impl PhyCalibrationTrackClass {
    /// Select only this TX class in a combined calibration request.
    pub const fn clients(self) -> PhyParamTrackRequest {
        match self {
            Self::Wifi => PhyParamTrackRequest::new(true, false),
            Self::BluetoothIeee802154 => PhyParamTrackRequest::new(false, true),
        }
    }

    /// Exact class selector used by the pinned vendor child.
    pub const fn selector(self) -> u8 {
        match self {
            Self::Wifi => 0,
            Self::BluetoothIeee802154 => 1,
        }
    }
}

/// Non-I/O arithmetic of `phy_temp_to_power_new` in the current PHY archive.
///
/// The function first truncates the temperature subtraction to a signed 16-bit
/// delta. Positive deltas use divisor eight for Wi-Fi and five for Bluetooth; zero and negative
/// deltas use divisor three for Wi-Fi and four for Bluetooth/IEEE 802.15.4.
/// The quotient becomes the protocol gain-base byte and is finally truncated
/// and sign-extended from eight bits.
pub const fn temperature_to_tracking_power(
    current_temperature: i16,
    reference_temperature: i16,
    class: PhyCalibrationTrackClass,
) -> i8 {
    let delta = current_temperature.wrapping_sub(reference_temperature);
    let divisor = match (class, delta > 0) {
        (PhyCalibrationTrackClass::Wifi, true) => 8,
        (PhyCalibrationTrackClass::BluetoothIeee802154, true) => 5,
        (PhyCalibrationTrackClass::Wifi, false) => 3,
        (PhyCalibrationTrackClass::BluetoothIeee802154, false) => 4,
    };
    (delta / divisor) as i8
}

/// One externally executed child of `phy_param_track_tot`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingAction {
    EnterCritical,
    RfpllCapTrack {
        diagnostics: PhyTrackingDiagnostics,
    },
    BluetoothIeee802154TxPowerTrack {
        enabled: bool,
        diagnostics: PhyTrackingDiagnostics,
    },
    CalibrationTrack {
        diagnostics: PhyTrackingDiagnostics,
        clients: PhyParamTrackRequest,
    },
    WifiI2cTrack,
    WifiTxPowerTrack {
        enabled: bool,
        diagnostics: PhyTrackingDiagnostics,
    },
    TemperatureRead,
    ExitCritical,
    Complete(PhyParamTrackingOutcome),
}

/// Identity-bound acknowledgement of one tracking action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingCompletion {
    EnteredCritical,
    RfpllCapTracked(PhyParamTrackingRfpllCompletion),
    /// `gain_updated`: the tracking gain base changed and was republished.
    BluetoothIeee802154TxPowerTracked {
        enabled: bool,
        gain_updated: bool,
    },
    CalibrationTracked(PhyParamTrackingCalibrationCompletion),
    WifiI2cTracked,
    WifiTxPowerTracked {
        enabled: bool,
        gain_updated: bool,
    },
    TemperatureRead,
    ExitedCritical,
}

/// Opaque proof that the complete RFPLL-cap child reached its terminal action
/// and committed its semantic reference-temperature outcome.
///
/// ```compile_fail
/// use oer_esp32s31_phy::tracking::parameters::{
///     PhyParamTrackingCompletion, PhyParamTrackingRfpllCompletion,
/// };
///
/// let forged = PhyParamTrackingCompletion::RfpllCapTracked(
///     PhyParamTrackingRfpllCompletion {
///         committed: (),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingRfpllCompletion {
    committed: (),
    /// A capacitance correction ran and committed a new reference temperature.
    corrected: bool,
}

/// Opaque proof that the selected calibration-tracking child reached its
/// unconditional TX-gain-compensation restore and committed its references.
///
/// ```compile_fail
/// use oer_esp32s31_phy::tracking::parameters::{
///     PhyCalibrationTrackClass, PhyParamTrackingCalibrationCompletion,
///     PhyParamTrackingCompletion,
/// };
///
/// let forged = PhyParamTrackingCompletion::CalibrationTracked(
///     PhyParamTrackingCalibrationCompletion {
///         clients: PhyCalibrationTrackClass::BluetoothIeee802154.clients(),
///         common_updated: true,
///         transmit_updated: true,
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingCalibrationCompletion {
    clients: PhyParamTrackRequest,
    common_updated: bool,
    transmit_updated: bool,
}

// Host tests and the vendor probe read back the committed branches;
// production consumes them through the parent transition.
#[cfg(any(test, oer_probe))]
impl PhyParamTrackingCalibrationCompletion {
    /// The common calibration references were recalibrated and committed.
    pub const fn common_updated(&self) -> bool {
        self.common_updated
    }

    /// The transmit calibration references were recalibrated and committed.
    pub const fn transmit_updated(&self) -> bool {
        self.transmit_updated
    }
}

/// Committed calibration branches, not merely invocation of their wrapper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CalibrationProgress {
    pub common: bool,
    /// The transmit references were committed, whichever clients requested
    /// tracking; `wifi` and `bluetooth_ieee802154` narrow it to requesters.
    pub transmit: bool,
    pub wifi: bool,
    pub bluetooth_ieee802154: bool,
}

/// TX-power tracking branches that changed and republished their gain base.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPowerProgress {
    pub wifi: bool,
    pub bluetooth_ieee802154: bool,
}

/// Observable outer-wrapper result. Child hardware postconditions are not
/// implied by this value; each action must be backed by its own transition.
/// Like the vendor progress word `phy_param_track_tot` returns, it reports
/// which children committed an update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingOutcome {
    pub clients: PhyParamTrackRequest,
    pub tracking_inhibited: bool,
    /// The RFPLL capacitance correction ran and committed its reference.
    pub rfpll_corrected: bool,
    pub tx_power: TxPowerProgress,
    pub calibration: CalibrationProgress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyParamTrackingStep {
    EnterCritical,
    RfpllCapTrack,
    BluetoothIeee802154TxPowerTrack,
    WifiI2cTrack,
    WifiTxPowerTrack,
    CalibrationTrack,
    TemperatureRead,
    ExitCritical,
    Complete,
}

/// Finite exact-order outer transition for one scheduler request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingTransition {
    request: PhyParamTrackRequest,
    policy: PhyParamTrackingPolicy,
    step: PhyParamTrackingStep,
    rfpll_corrected: bool,
    tx_power: TxPowerProgress,
    calibration: CalibrationProgress,
}

impl PhyParamTrackingTransition {
    pub(crate) const fn new(request: PhyParamTrackRequest, policy: PhyParamTrackingPolicy) -> Self {
        Self {
            request,
            policy,
            step: PhyParamTrackingStep::EnterCritical,
            rfpll_corrected: false,
            tx_power: TxPowerProgress {
                wifi: false,
                bluetooth_ieee802154: false,
            },
            calibration: CalibrationProgress {
                common: false,
                transmit: false,
                wifi: false,
                bluetooth_ieee802154: false,
            },
        }
    }

    pub const fn action(self) -> PhyParamTrackingAction {
        match self.step {
            PhyParamTrackingStep::EnterCritical => PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingStep::RfpllCapTrack => PhyParamTrackingAction::RfpllCapTrack {
                diagnostics: self.policy.diagnostics,
            },
            PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack => {
                PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                    enabled: self.policy.bluetooth_ieee802154_power_tracking_enabled,
                    diagnostics: self.policy.diagnostics,
                }
            }
            PhyParamTrackingStep::WifiI2cTrack => PhyParamTrackingAction::WifiI2cTrack,
            PhyParamTrackingStep::WifiTxPowerTrack => PhyParamTrackingAction::WifiTxPowerTrack {
                enabled: true,
                diagnostics: self.policy.diagnostics,
            },
            PhyParamTrackingStep::CalibrationTrack => PhyParamTrackingAction::CalibrationTrack {
                diagnostics: self.policy.diagnostics,
                clients: self.request,
            },
            PhyParamTrackingStep::TemperatureRead => PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingStep::ExitCritical => PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingStep::Complete => {
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: self.request,
                    tracking_inhibited: self.policy.tracking_inhibited,
                    rfpll_corrected: self.rfpll_corrected,
                    tx_power: self.tx_power,
                    calibration: self.calibration,
                })
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyParamTrackingCompletion,
    ) -> Result<(), PhyParamTrackingTransitionError> {
        let next = match (self.step, completion) {
            (PhyParamTrackingStep::EnterCritical, PhyParamTrackingCompletion::EnteredCritical) => {
                if self.policy.tracking_inhibited {
                    PhyParamTrackingStep::ExitCritical
                } else if self.policy.rfpll_cap_tracking_enabled {
                    PhyParamTrackingStep::RfpllCapTrack
                } else {
                    self.first_client_step()
                }
            }
            (
                PhyParamTrackingStep::RfpllCapTrack,
                PhyParamTrackingCompletion::RfpllCapTracked(completion),
            ) => {
                self.rfpll_corrected = completion.corrected;
                self.first_client_step()
            }
            (
                PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack,
                PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                    enabled,
                    gain_updated,
                },
            ) if enabled == self.policy.bluetooth_ieee802154_power_tracking_enabled => {
                self.tx_power.bluetooth_ieee802154 = gain_updated;
                self.first_wifi_step()
            }
            (PhyParamTrackingStep::WifiI2cTrack, PhyParamTrackingCompletion::WifiI2cTracked) => {
                PhyParamTrackingStep::WifiTxPowerTrack
            }
            (
                PhyParamTrackingStep::WifiTxPowerTrack,
                PhyParamTrackingCompletion::WifiTxPowerTracked {
                    enabled: true,
                    gain_updated,
                },
            ) => {
                self.tx_power.wifi = gain_updated;
                self.first_calibration_step()
            }
            (
                PhyParamTrackingStep::CalibrationTrack,
                PhyParamTrackingCompletion::CalibrationTracked(completion),
            ) if completion.clients == self.request => {
                self.calibration.common = completion.common_updated;
                self.calibration.transmit = completion.transmit_updated;
                self.calibration.wifi = completion.transmit_updated && self.request.wifi();
                self.calibration.bluetooth_ieee802154 =
                    completion.transmit_updated && self.request.bluetooth_ieee802154();
                PhyParamTrackingStep::TemperatureRead
            }
            (
                PhyParamTrackingStep::TemperatureRead,
                PhyParamTrackingCompletion::TemperatureRead,
            ) => PhyParamTrackingStep::ExitCritical,
            (PhyParamTrackingStep::ExitCritical, PhyParamTrackingCompletion::ExitedCritical) => {
                PhyParamTrackingStep::Complete
            }
            (PhyParamTrackingStep::Complete, _) => {
                return Err(PhyParamTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyParamTrackingTransitionError::WrongCompletion),
        };
        self.step = next;
        Ok(())
    }

    /// Lower only the RFPLL-cap action and retain exclusive access to the live
    /// reference temperature until the complete child is committed.
    pub fn begin_rfpll_cap_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingRfpllTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingRfpllTransition::lower(self.action(), self.policy, state)
    }

    /// Lower only the selected calibration action and retain its two live
    /// semantic temperature references until terminal commit.
    pub fn begin_calibration_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingCalibrationTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingCalibrationTransition::lower(self.action(), self.policy, state)
    }

    /// Lower only the currently selected TX-power child, using the immutable
    /// parameter snapshot captured with this outer transition.
    pub fn begin_tx_power_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingTxPowerTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingTxPowerTransition::lower(self.action(), self.policy, state)
    }

    /// Lower only the Wi-Fi PHY-I2C child while retaining exclusive access to
    /// the live range state committed after both masked writes complete.
    pub fn begin_wifi_i2c_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingWifiI2cTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingWifiI2cTransition::lower(self.action(), state)
    }

    /// Lower only the final temperature child while retaining exclusive access
    /// to the live state that will receive its terminal outcome.
    pub fn begin_temperature_read<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingTemperatureTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingTemperatureTransition::lower(self.action(), state)
    }

    const fn first_client_step(self) -> PhyParamTrackingStep {
        if self.request.bluetooth_ieee802154 {
            PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack
        } else {
            self.first_wifi_step()
        }
    }

    const fn first_wifi_step(self) -> PhyParamTrackingStep {
        if self.request.wifi {
            PhyParamTrackingStep::WifiI2cTrack
        } else {
            self.first_calibration_step()
        }
    }

    const fn first_calibration_step(self) -> PhyParamTrackingStep {
        if self.policy.calibration_tracking_enabled {
            PhyParamTrackingStep::CalibrationTrack
        } else {
            PhyParamTrackingStep::TemperatureRead
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingChildError {
    UnsupportedAction,
}

/// Complete RFPLL-cap child selected by the outer periodic transition.
///
/// The corresponding outer completion contains an opaque token. It can only
/// be minted by [`Self::commit`] after the child has restored hardware
/// frequency control and published its terminal outcome.
pub struct PhyParamTrackingRfpllTransition<'state> {
    parent_action: PhyParamTrackingAction,
    child: super::rfpll::thermal::Transition,
    state: &'state mut crate::state::PhyState,
}

impl fmt::Debug for PhyParamTrackingRfpllTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingRfpllTransition")
            .field("parent_action", &self.parent_action)
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingRfpllTransition<'state> {
    fn lower(
        parent_action: PhyParamTrackingAction,
        policy: PhyParamTrackingPolicy,
        state: &'state mut crate::state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        if !matches!(parent_action, PhyParamTrackingAction::RfpllCapTrack { .. }) {
            return Err(PhyParamTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            parent_action,
            child: super::rfpll::thermal::Transition::new(
                state.rfpll_tracking_request(policy.rfpll_cap_tracking_threshold),
            ),
            state,
        })
    }

    #[cfg(any(test, oer_probe))]
    pub const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }

    pub const fn request(&self) -> super::rfpll::thermal::Request {
        self.child.request()
    }

    #[inline(always)]
    pub const fn action(&self) -> super::rfpll::thermal::Action {
        self.child.action()
    }

    #[cfg(any(test, oer_probe))]
    #[inline(always)]
    pub fn advance(
        &mut self,
        completion: super::rfpll::thermal::Completion,
    ) -> Result<(), super::rfpll::Error> {
        self.child.advance(completion)
    }

    pub(crate) fn transition_mut(&mut self) -> &mut super::rfpll::thermal::Transition {
        &mut self.child
    }

    pub const fn state(&self) -> &crate::state::PhyState {
        self.state
    }

    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let super::rfpll::thermal::Action::Complete(outcome) = self.child.action() else {
            return Err(self);
        };
        let PhyParamTrackingAction::RfpllCapTrack { .. } = self.parent_action else {
            unreachable!()
        };
        self.state.commit_rfpll_tracking(outcome);
        Ok(PhyParamTrackingCompletion::RfpllCapTracked(
            PhyParamTrackingRfpllCompletion {
                committed: (),
                corrected: outcome.correction.is_some(),
            },
        ))
    }
}

/// Complete calibration child selected by the outer periodic transition.
///
/// The owner holds exclusive access to both semantic calibration
/// references. It can mint the corresponding outer completion only after the
/// child reaches its terminal action and commits its outcome.
pub struct PhyParamTrackingCalibrationTransition<'state> {
    parent_action: PhyParamTrackingAction,
    child: crate::tracking::calibration::PhyCalibrationTrackingTransition,
    state: &'state mut crate::state::PhyState,
}

impl fmt::Debug for PhyParamTrackingCalibrationTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingCalibrationTransition")
            .field("parent_action", &self.parent_action)
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingCalibrationTransition<'state> {
    pub(crate) fn lower(
        parent_action: PhyParamTrackingAction,
        policy: PhyParamTrackingPolicy,
        state: &'state mut crate::state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        let PhyParamTrackingAction::CalibrationTrack { clients, .. } = parent_action else {
            return Err(PhyParamTrackingChildError::UnsupportedAction);
        };
        Ok(Self {
            parent_action,
            child: crate::tracking::calibration::PhyCalibrationTrackingTransition::start(
                crate::tracking::calibration::PhyCalibrationTrackingRequest { clients },
                state.calibration_tracking_parameters(policy.calibration_tracking_threshold),
            ),
            state,
        })
    }

    #[cfg(any(test, oer_probe))]
    pub const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }

    pub const fn action(&self) -> crate::tracking::calibration::PhyCalibrationTrackingAction {
        self.child.action()
    }

    /// The calibration branch of the current action.
    pub const fn branch(
        &self,
    ) -> Option<crate::tracking::calibration::PhyCalibrationTrackingBranch> {
        self.child.branch()
    }

    pub fn advance(
        &mut self,
        completion: crate::tracking::calibration::PhyCalibrationTrackingCompletion,
    ) -> Result<(), crate::tracking::calibration::PhyCalibrationTrackingTransitionError> {
        self.child.advance(completion)
    }

    pub fn begin_force_txrx(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationForceTxRxTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child.begin_force_txrx()
    }

    pub fn begin_pbus_clear(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationPbusClearTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child.begin_pbus_clear()
    }

    pub fn begin_dcode(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationDcodeTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child.begin_dcode()
    }

    pub fn begin_rx_gain_recalibration(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationRxGainTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child
            .begin_rx_gain_recalibration(self.state.rx_gain_init_parameters())
    }

    pub fn begin_channel_restore(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationChannelTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child
            .begin_channel_restore(self.state.channel_parameters())
    }

    pub fn begin_tx_dc_pwdet(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationTxDcPwdetTransition,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child.begin_tx_dc_pwdet(
            self.state.tx_dc_pwdet_parameters(),
            self.state.bluetooth_tx_dc_pwdet_transition(),
        )
    }

    pub fn begin_tx_gain_publication(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationTxGainBinding,
        crate::tracking::calibration::PhyCalibrationTrackingChildError,
    > {
        self.child.begin_tx_gain_publication(self.state)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::tracking::calibration::PhyCalibrationTrackingExternalBinding,
        crate::tracking::calibration::PhyCalibrationTrackingBindingError,
    > {
        crate::tracking::calibration::PhyCalibrationTrackingExternalBinding::lower(self.action())
    }

    #[cfg(any(test, oer_probe))]
    pub const fn state(&self) -> &crate::state::PhyState {
        self.state
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must retain the allocation-free calibration outcome and live state owner"
    )]
    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::tracking::calibration::PhyCalibrationTrackingAction::Complete(outcome) =
            self.child.action()
        else {
            return Err(self);
        };
        let PhyParamTrackingAction::CalibrationTrack { clients, .. } = self.parent_action else {
            unreachable!()
        };
        self.state.apply_calibration_tracking_outcome(outcome);
        Ok(PhyParamTrackingCompletion::CalibrationTracked(
            PhyParamTrackingCalibrationCompletion {
                clients,
                common_updated: outcome.common_updated,
                transmit_updated: outcome.transmit_updated,
            },
        ))
    }
}

/// One exact TX-power child selected by the outer tracking transition.
///
/// This owner deliberately exposes no way to manufacture the corresponding
/// outer completion. The child must first reach its terminal action and commit
/// that outcome into the same live [`crate::state::PhyState`].
pub struct PhyParamTrackingTxPowerTransition<'state> {
    parent_action: PhyParamTrackingAction,
    child: crate::tracking::power::PhyTxPowerTrackingTransition,
    state: &'state mut crate::state::PhyState,
}

impl fmt::Debug for PhyParamTrackingTxPowerTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingTxPowerTransition")
            .field("parent_action", &self.parent_action)
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingTxPowerTransition<'state> {
    fn lower(
        parent_action: PhyParamTrackingAction,
        policy: PhyParamTrackingPolicy,
        state: &'state mut crate::state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        let request = match parent_action {
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { enabled, .. } => {
                crate::tracking::power::PhyTxPowerTrackingRequest {
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                    enabled,
                    wifi_channel: state.current_wifi_channel(),
                }
            }
            PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
                crate::tracking::power::PhyTxPowerTrackingRequest {
                    class: PhyCalibrationTrackClass::Wifi,
                    enabled,
                    wifi_channel: state.current_wifi_channel(),
                }
            }
            _ => return Err(PhyParamTrackingChildError::UnsupportedAction),
        };
        Ok(Self {
            parent_action,
            child: crate::tracking::power::PhyTxPowerTrackingTransition::new(
                request,
                state.tx_power_tracking_parameters(policy.relaxed_power_tracking_threshold),
            ),
            state,
        })
    }

    #[cfg(any(test, oer_probe))]
    pub const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }

    pub const fn action(&self) -> crate::tracking::power::PhyTxPowerTrackingAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::tracking::power::PhyTxPowerTrackingCompletion,
    ) -> Result<(), crate::tracking::power::PhyTxPowerTrackingTransitionError> {
        self.child.advance(completion)
    }

    /// Bind the current child action to the existing target HAL/PAC edge while
    /// the exact live state remains exclusively borrowed by this transaction.
    pub fn lower_external(
        &self,
    ) -> Result<
        crate::tracking::power::PhyTxPowerTrackingExternalBinding,
        crate::tracking::power::PhyTxPowerTrackingBindingError,
    > {
        crate::tracking::power::PhyTxPowerTrackingExternalBinding::lower(self.action(), self.state)
    }

    #[cfg(any(test, oer_probe))]
    pub const fn state(&self) -> &crate::state::PhyState {
        self.state
    }

    /// Commit the terminal child result and mint its identity-bound parent
    /// completion. An incomplete child is returned unchanged.
    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::tracking::power::PhyTxPowerTrackingAction::Complete(outcome) =
            self.child.action()
        else {
            return Err(self);
        };
        self.state.apply_tx_power_tracking_outcome(outcome);
        let gain_updated = outcome.gain_updated;
        Ok(match self.parent_action {
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { enabled, .. } => {
                PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                    enabled,
                    gain_updated,
                }
            }
            PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
                PhyParamTrackingCompletion::WifiTxPowerTracked {
                    enabled,
                    gain_updated,
                }
            }
            _ => unreachable!(),
        })
    }
}

/// Wi-Fi PHY-I2C child selected by the outer periodic transition.
///
/// The child owns both read/modify/write transactions. It cannot mint the
/// parent's `WifiI2cTracked` acknowledgement until both writes finish and the
/// new semantic temperature band is committed to the same live PHY state.
pub struct PhyParamTrackingWifiI2cTransition<'state> {
    child: crate::tracking::i2c::PhyWifiI2cTrackingTransition,
    state: &'state mut crate::state::PhyState,
}

impl fmt::Debug for PhyParamTrackingWifiI2cTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingWifiI2cTransition")
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingWifiI2cTransition<'state> {
    fn lower(
        parent_action: PhyParamTrackingAction,
        state: &'state mut crate::state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        if parent_action != PhyParamTrackingAction::WifiI2cTrack {
            return Err(PhyParamTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::tracking::i2c::PhyWifiI2cTrackingTransition::new(
                state.wifi_i2c_tracking_parameters(),
            ),
            state,
        })
    }

    #[cfg(any(test, oer_probe))]
    pub const fn action(&self) -> crate::tracking::i2c::PhyWifiI2cTrackingAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::tracking::i2c::PhyWifiI2cTrackingCompletion,
    ) -> Result<(), crate::tracking::i2c::PhyWifiI2cTrackingTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::analog::i2c::MaskedI2cWriteBinding,
        crate::analog::i2c::MaskedI2cWriteBindingError,
    > {
        self.child.lower_external()
    }

    #[cfg(any(test, oer_probe))]
    pub const fn state(&self) -> &crate::state::PhyState {
        self.state
    }

    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::tracking::i2c::PhyWifiI2cTrackingAction::Complete(outcome) = self.child.action()
        else {
            return Err(self);
        };
        self.state.apply_wifi_i2c_tracking_outcome(outcome);
        Ok(PhyParamTrackingCompletion::WifiI2cTracked)
    }
}

/// Final temperature child selected by the outer periodic transition.
///
/// A failed or incomplete sensor transaction retains this owner and cannot
/// mint `TemperatureRead`; the caller must poison the outer scheduler epoch.
pub struct PhyParamTrackingTemperatureTransition<'state> {
    child: crate::analog::temperature::PhyTemperatureTransition,
    state: &'state mut crate::state::PhyState,
}

impl fmt::Debug for PhyParamTrackingTemperatureTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingTemperatureTransition")
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingTemperatureTransition<'state> {
    pub(crate) fn transition_mut(
        &mut self,
    ) -> &mut crate::analog::temperature::PhyTemperatureTransition {
        &mut self.child
    }

    fn lower(
        parent_action: PhyParamTrackingAction,
        state: &'state mut crate::state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        if parent_action != PhyParamTrackingAction::TemperatureRead {
            return Err(PhyParamTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::analog::temperature::PhyTemperatureTransition::new(),
            state,
        })
    }

    pub const fn action(&self) -> crate::analog::temperature::PhyTemperatureAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::analog::temperature::PhyTemperatureCompletion,
    ) -> Result<(), crate::analog::temperature::PhyTemperatureTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::analog::temperature::PhyTemperatureExternalBinding,
        crate::analog::temperature::PhyTemperatureBindingError,
    > {
        crate::analog::temperature::PhyTemperatureExternalBinding::lower(self.action())
    }

    #[cfg(any(test, oer_probe))]
    pub const fn state(&self) -> &crate::state::PhyState {
        self.state
    }

    /// Commit a successful sensor outcome and mint the exact parent
    /// completion. Failed and incomplete children are returned unchanged.
    #[cfg(any(test, oer_probe))]
    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        self.commit_observed(None, None)
    }

    pub(crate) fn commit_observed(
        self,
        started: Option<u64>,
        completed: Option<u64>,
    ) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::analog::temperature::PhyTemperatureAction::Complete(outcome) =
            self.child.action()
        else {
            return Err(self);
        };
        self.state
            .apply_observed_temperature_outcome(outcome, started, completed);
        Ok(PhyParamTrackingCompletion::TemperatureRead)
    }
}

#[cfg(test)]
mod tests;
