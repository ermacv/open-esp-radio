//! Pure shared-PHY client-set and PLL-tracking scheduler model.
//!
//! The reviewed ESP-IDF software protocol stores Wi-Fi, Bluetooth, and IEEE
//! 802.15.4 as independent bits. It gives Wi-Fi one tracking timestamp and
//! Bluetooth plus IEEE 802.15.4 a shared timestamp. This module reproduces only
//! those software decisions. It performs no MMIO and does not arm a real
//! timer. A due request retains the unique owner while the exact outer
//! parameter-tracking transition is executed.
//! Its recovered TX-power, Wi-Fi PHY-I2C, calibration, RFPLL and temperature
//! children compose into live-state owners; calibration hardware effects stay
//! explicit unresolved actions until their target bindings are owned.
//!
//! Timer arm/stop and the PHY lock are represented only as atomic model facts.
//! There is no fallible timer executor, rollback protocol, or target lock in
//! this module, so the resulting transition is not an RF-readiness proof.
//! Clock sampling is nevertheless exact: immediate evaluation preserves the
//! public source's short-circuit due checks and then samples each active class
//! again while refreshing timestamps; the periodic callback performs only the
//! per-class refresh samples.

use core::fmt;

use crate::tracking::parameters::{
    PhyParamTrackRequest, PhyParamTrackingAction, PhyParamTrackingCalibrationTransition,
    PhyParamTrackingChildError, PhyParamTrackingCompletion, PhyParamTrackingPolicy,
    PhyParamTrackingRfpllTransition, PhyParamTrackingTemperatureTransition,
    PhyParamTrackingTransition, PhyParamTrackingTransitionError, PhyParamTrackingTxPowerTransition,
    PhyParamTrackingWifiI2cTransition,
};

const WIFI_BIT: u8 = 1;
const BLUETOOTH_BIT: u8 = 2;
const IEEE802154_BIT: u8 = 4;
const VALID_CLIENT_BITS: u8 = WIFI_BIT | BLUETOOTH_BIT | IEEE802154_BIT;

/// Source-reviewed default periodic PLL-tracking interval.
pub const DEFAULT_PLL_TRACK_PERIOD_MICROS: u64 = 1_000_000;

/// One typed user of the shared PHY software client set: the portable
/// radio client.
pub use oer_esp32s31_hal::shared_radio::RadioClient;

/// The bit of `client` in the reviewed software client set.
pub(crate) const fn client_bit(client: RadioClient) -> u8 {
    match client {
        RadioClient::Wifi => WIFI_BIT,
        RadioClient::Bluetooth => BLUETOOTH_BIT,
        RadioClient::Ieee802154 => IEEE802154_BIT,
    }
}

/// Tracking timestamp class selected by the reviewed scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPllTrackClass {
    Wifi,
    BluetoothIeee802154,
}

/// Copyable observation of software state without a raw-mask mutation path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyClientSnapshot {
    wifi: bool,
    bluetooth: bool,
    ieee802154: bool,
    tracker_model_armed: bool,
    wifi_previous_micros: u64,
    bluetooth_ieee802154_previous_micros: u64,
    period_micros: u64,
}

impl PhyClientSnapshot {
    pub const fn contains(self, client: RadioClient) -> bool {
        match client {
            RadioClient::Wifi => self.wifi,
            RadioClient::Bluetooth => self.bluetooth,
            RadioClient::Ieee802154 => self.ieee802154,
        }
    }

    pub const fn is_empty(self) -> bool {
        !self.wifi && !self.bluetooth && !self.ieee802154
    }

    /// Whether the pure model carries the first-client timer obligation.
    ///
    /// This is not evidence that a target timer was created or started.
    pub const fn tracker_model_armed(self) -> bool {
        self.tracker_model_armed
    }

    pub const fn previous_micros(self, class: PhyPllTrackClass) -> u64 {
        match class {
            PhyPllTrackClass::Wifi => self.wifi_previous_micros,
            PhyPllTrackClass::BluetoothIeee802154 => self.bluetooth_ieee802154_previous_micros,
        }
    }

    pub const fn period_micros(self) -> u64 {
        self.period_micros
    }

    /// Earliest absolute deadline for `evaluate_immediate_tracking`.
    ///
    /// The reviewed due predicate is strictly greater than the interval, so
    /// the first due microsecond is `previous + period + 1`. Inactive classes
    /// do not arm a timer. An unrepresentable deadline is an error, not an
    /// immediately due timer or a silently disabled tracker.
    pub fn next_tracking_deadline_micros(self) -> Result<Option<u64>, PhyTrackTimeError> {
        let mut earliest = None;
        for (active, class, previous) in [
            (self.wifi, PhyPllTrackClass::Wifi, self.wifi_previous_micros),
            (
                self.bluetooth || self.ieee802154,
                PhyPllTrackClass::BluetoothIeee802154,
                self.bluetooth_ieee802154_previous_micros,
            ),
        ] {
            if !active {
                continue;
            }
            let deadline = previous
                .checked_add(self.period_micros)
                .and_then(|value| value.checked_add(1))
                .ok_or(PhyTrackTimeError::DeadlineOverflow { class })?;
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        Ok(earliest)
    }
}

/// Unique owner of the source-reviewed software client and scheduler state.
///
/// This type is deliberately neither `Clone` nor `Copy`. Its private bit image
/// cannot be supplied or changed by safe callers. It is not a hardware lease or
/// an RF/PLL readiness proof.
#[must_use = "the PHY client model owns its unique software state"]
pub struct PhyClientState {
    bits: u8,
    tracker_model_armed: bool,
    // Registration that minted this client set. It occupies alignment padding
    // beside the flags, so detached owners carry it without growing.
    epoch: Option<oer_esp32s31_hal::owner::PhyRegistrationEpoch>,
    wifi_previous_micros: u64,
    bluetooth_ieee802154_previous_micros: u64,
    period_micros: u64,
}

impl PhyClientState {
    /// Mint an empty scheduler for one freshly registered hardware epoch.
    ///
    /// This constructor is crate-controlled so application code cannot create
    /// an unrelated client mask and present it beside registered hardware.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn for_registration(
        period_micros: u64,
        epoch: oer_esp32s31_hal::owner::PhyRegistrationEpoch,
    ) -> Self {
        Self::empty(period_micros, Some(epoch))
    }

    /// Mint an empty model with no registration identity.
    ///
    /// Host models and validation fixtures use this constructor. The result
    /// never describes a hardware partition, so it cannot authorize a hardware
    /// operation that checks [`Self::describes`].
    #[cfg(any(test, feature = "validation-probes"))]
    pub(crate) const fn without_registration(period_micros: u64) -> Self {
        Self::empty(period_micros, None)
    }

    #[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
    const fn empty(
        period_micros: u64,
        epoch: Option<oer_esp32s31_hal::owner::PhyRegistrationEpoch>,
    ) -> Self {
        Self {
            epoch,
            bits: 0,
            tracker_model_armed: false,
            wifi_previous_micros: 0,
            bluetooth_ieee802154_previous_micros: 0,
            period_micros,
        }
    }

    /// Mint an empty model at the crate-controlled physical-epoch boundary.
    ///
    /// The caller must not use this constructor as evidence about a target
    /// global or vendor-owned client mask.
    #[cfg(test)]
    const fn new_empty(period_micros: u64) -> Self {
        Self::without_registration(period_micros)
    }

    /// Whether the registration that minted this client set still describes
    /// the borrowed PHY partition.
    ///
    /// Owners that travel apart from their hardware check this before every
    /// hardware operation. A later registration on the same partition, or a
    /// return of the partition to the cold radio root, makes it false. A model
    /// without registration identity never describes hardware.
    pub(crate) fn describes(
        &self,
        hardware: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> bool {
        self.describes_epoch(hardware.registration_epoch())
    }

    /// Whether this client set was minted by exactly `epoch`.
    ///
    /// Detached hardware owners that are not borrowable shared-PHY partitions,
    /// such as the retained radio root, report their epoch directly.
    pub(crate) fn describes_epoch(
        &self,
        epoch: Option<oer_esp32s31_hal::owner::PhyRegistrationEpoch>,
    ) -> bool {
        self.epoch.is_some() && epoch == self.epoch
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        PhyClientSnapshot {
            wifi: self.bits & WIFI_BIT != 0,
            bluetooth: self.bits & BLUETOOTH_BIT != 0,
            ieee802154: self.bits & IEEE802154_BIT != 0,
            tracker_model_armed: self.tracker_model_armed,
            wifi_previous_micros: self.wifi_previous_micros,
            bluetooth_ieee802154_previous_micros: self.bluetooth_ieee802154_previous_micros,
            period_micros: self.period_micros,
        }
    }

    /// Consume the owner and acquire one client bit.
    ///
    /// Duplicate acquisition is rejected before any model state changes. A
    /// first aggregate acquisition records the reviewed arm-before-set-before-
    /// evaluate ordering. Later acquisitions record set-before-evaluate. The
    /// arm is an infallible model fact, not evidence of a target timer.
    pub fn acquire(
        mut self,
        client: RadioClient,
        clock: &impl oer_time::Clock,
    ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure> {
        let bit = client_bit(client);
        if self.bits & bit != 0 {
            return Err(PhyClientAcquireFailure {
                owner: self,
                error: PhyClientAcquireError::AlreadyAcquired(client),
            });
        }

        let was_empty = self.bits == 0;
        let prospective_bits = self.bits | bit;
        let evaluation = match self.evaluate_for_bits(prospective_bits, clock) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                return Err(PhyClientAcquireFailure {
                    owner: self,
                    error: PhyClientAcquireError::TrackingTime(error),
                });
            }
        };

        if was_empty {
            self.tracker_model_armed = true;
        }
        self.bits = prospective_bits;
        self.apply_evaluation(evaluation);

        debug_assert_eq!(self.bits & !VALID_CLIENT_BITS, 0);
        debug_assert_eq!(self.tracker_model_armed, self.bits != 0);

        Ok(PhyClientAcquireOutcome {
            client,
            was_empty,
            ordering: if was_empty {
                PhyClientAcquireOrdering::ArmThenSetThenEvaluate
            } else {
                PhyClientAcquireOrdering::SetThenEvaluate
            },
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    /// Consume the owner and release one client bit.
    ///
    /// Missing release is rejected without changing the owner. `is_last`
    /// matches the reviewed saved-mask equality test; unrelated bits survive.
    /// Last-user timer stop is an infallible model fact here, not a target
    /// executor or rollback contract.
    pub fn release(
        mut self,
        client: RadioClient,
    ) -> Result<PhyClientReleaseOutcome, PhyClientReleaseFailure> {
        let bit = client_bit(client);
        if self.bits & bit == 0 {
            return Err(PhyClientReleaseFailure {
                owner: self,
                error: PhyClientReleaseError::NotAcquired(client),
            });
        }

        let saved_bits = self.bits;
        self.bits &= !bit;
        let is_last = saved_bits == bit;
        if is_last {
            self.tracker_model_armed = false;
        }

        debug_assert_eq!(self.bits & !VALID_CLIENT_BITS, 0);
        debug_assert_eq!(self.tracker_model_armed, self.bits != 0);

        Ok(PhyClientReleaseOutcome {
            owner: self,
            client,
            is_last,
        })
    }

    /// Apply the reviewed immediate-on-enable due check without performing PLL
    /// hardware work.
    pub fn evaluate_immediate_tracking(
        mut self,
        clock: &impl oer_time::Clock,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        let evaluation = match self.evaluate_for_bits(self.bits, clock) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                return Err(PhyTrackEvaluationFailure { owner: self, error });
            }
        };
        self.apply_evaluation(evaluation);
        Ok(PhyTrackEvaluation {
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    /// Apply one reviewed periodic callback without performing PLL hardware
    /// work.
    ///
    /// Unlike [`Self::evaluate_immediate_tracking`], the timer callback invokes
    /// the internal scheduler unconditionally. An active class therefore emits
    /// a request and refreshes its timestamp on every callback.
    pub fn evaluate_periodic_tracking(
        mut self,
        clock: &impl oer_time::Clock,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        let wifi_active = self.bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = self.bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;
        let evaluation = match self.build_evaluation(
            wifi_active,
            bluetooth_ieee802154_active,
            wifi_active || bluetooth_ieee802154_active,
            clock,
        ) {
            Ok(evaluation) => evaluation,
            Err(error) => return Err(PhyTrackEvaluationFailure { owner: self, error }),
        };
        self.apply_evaluation(evaluation);
        Ok(PhyTrackEvaluation {
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    fn evaluate_for_bits(
        &self,
        bits: u8,
        clock: &impl oer_time::Clock,
    ) -> Result<TrackEvaluation, PhyTrackTimeError> {
        let wifi_active = bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;

        let mut request_due = false;
        if wifi_active {
            let now_micros = clock.now().as_micros();
            self.validate_timestamp(
                PhyPllTrackClass::Wifi,
                self.wifi_previous_micros,
                now_micros,
            )?;
            request_due = now_micros - self.wifi_previous_micros > self.period_micros;
        }
        // Preserve the two source assignments containing `need_track_pll ||`.
        // Once Wi-Fi is due, C short-circuiting skips the BT/154 due sample.
        if bluetooth_ieee802154_active && !request_due {
            let now_micros = clock.now().as_micros();
            self.validate_timestamp(
                PhyPllTrackClass::BluetoothIeee802154,
                self.bluetooth_ieee802154_previous_micros,
                now_micros,
            )?;
            request_due =
                now_micros - self.bluetooth_ieee802154_previous_micros > self.period_micros;
        }

        self.build_evaluation(wifi_active, bluetooth_ieee802154_active, request_due, clock)
    }

    fn build_evaluation(
        &self,
        wifi_active: bool,
        bluetooth_ieee802154_active: bool,
        request_due: bool,
        clock: &impl oer_time::Clock,
    ) -> Result<TrackEvaluation, PhyTrackTimeError> {
        if !request_due {
            return Ok(TrackEvaluation {
                wifi_active,
                bluetooth_ieee802154_active,
                wifi_refresh_micros: None,
                bluetooth_ieee802154_refresh_micros: None,
            });
        }

        let wifi_refresh_micros = if wifi_active {
            let now_micros = clock.now().as_micros();
            self.validate_timestamp(
                PhyPllTrackClass::Wifi,
                self.wifi_previous_micros,
                now_micros,
            )?;
            Some(now_micros)
        } else {
            None
        };
        let bluetooth_ieee802154_refresh_micros = if bluetooth_ieee802154_active {
            let now_micros = clock.now().as_micros();
            self.validate_timestamp(
                PhyPllTrackClass::BluetoothIeee802154,
                self.bluetooth_ieee802154_previous_micros,
                now_micros,
            )?;
            Some(now_micros)
        } else {
            None
        };

        Ok(TrackEvaluation {
            wifi_active,
            bluetooth_ieee802154_active,
            wifi_refresh_micros,
            bluetooth_ieee802154_refresh_micros,
        })
    }

    fn validate_timestamp(
        &self,
        class: PhyPllTrackClass,
        previous_micros: u64,
        now_micros: u64,
    ) -> Result<(), PhyTrackTimeError> {
        if now_micros < previous_micros {
            Err(PhyTrackTimeError::TimeReversed {
                class,
                previous_micros,
                now_micros,
            })
        } else {
            Ok(())
        }
    }

    fn apply_evaluation(&mut self, evaluation: TrackEvaluation) {
        if let Some(now_micros) = evaluation.wifi_refresh_micros {
            self.wifi_previous_micros = now_micros;
        }
        if let Some(now_micros) = evaluation.bluetooth_ieee802154_refresh_micros {
            self.bluetooth_ieee802154_previous_micros = now_micros;
        }
    }
}

#[derive(Clone, Copy)]
struct TrackEvaluation {
    wifi_active: bool,
    bluetooth_ieee802154_active: bool,
    wifi_refresh_micros: Option<u64>,
    bluetooth_ieee802154_refresh_micros: Option<u64>,
}

impl TrackEvaluation {
    fn request(self) -> Option<PhyParamTrackRequest> {
        (self.wifi_refresh_micros.is_some() || self.bluetooth_ieee802154_refresh_micros.is_some())
            .then_some(PhyParamTrackRequest::new(
                self.wifi_active,
                self.bluetooth_ieee802154_active,
            ))
    }
}

enum TrackContinuation {
    Settled(PhyClientState),
    Pending(PhyPendingTrack),
}

impl TrackContinuation {
    fn new(owner: PhyClientState, evaluation: TrackEvaluation) -> Self {
        match evaluation.request() {
            Some(request) => Self::Pending(PhyPendingTrack { owner, request }),
            None => Self::Settled(owner),
        }
    }

    const fn request(&self) -> Option<&PhyParamTrackRequest> {
        match self {
            Self::Settled(_) => None,
            Self::Pending(pending) => Some(pending.request()),
        }
    }

    const fn owner(&self) -> &PhyClientState {
        match self {
            Self::Settled(owner) => owner,
            Self::Pending(pending) => &pending.owner,
        }
    }

    fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        match self {
            Self::Settled(owner) => Ok(owner),
            Self::Pending(pending) => Err(pending),
        }
    }
}

/// Reviewed ordering of one successful acquisition's software obligations.
///
/// These variants describe atomic pure-model facts; they are not timer or lock
/// completion tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientAcquireOrdering {
    ArmThenSetThenEvaluate,
    SetThenEvaluate,
}

/// Owner held while the source-reviewed tracking request remains unresolved.
///
/// Safe code can inspect the request but cannot recover or operate on the
/// client owner. It can begin the exact outer tracking transition or poison
/// the epoch explicitly.
#[must_use = "the tracking request retains the unique PHY client owner"]
pub struct PhyPendingTrack {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
}

impl fmt::Debug for PhyPendingTrack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyPendingTrack")
            .field("wifi", &self.request.wifi())
            .field("bluetooth_ieee802154", &self.request.bluetooth_ieee802154())
            .finish_non_exhaustive()
    }
}

impl PhyPendingTrack {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        &self.request
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }

    /// Bind a registered-epoch policy to the exact outer tracking transition.
    pub(crate) fn begin_tracking(self, policy: PhyParamTrackingPolicy) -> PhyPendingTracking {
        PhyPendingTracking {
            owner: self.owner,
            request: self.request,
            transition: PhyParamTrackingTransition::new(self.request, policy),
        }
    }

    /// Explicitly poison a model request that target hardware did not complete.
    pub fn fail(self) -> PhyTrackPoisoned {
        PhyTrackPoisoned {
            owner: self.owner,
            request: self.request,
        }
    }

    #[cfg(test)]
    fn complete_for_test(self) -> PhyClientState {
        self.owner
    }
}

/// Unique client owner retained through one exact outer tracking transition.
///
/// Wrong completions preserve both this owner and the current action. Ordinary
/// access is restored only after the terminal action is visible.
#[must_use = "the in-flight tracking transition retains the unique PHY client owner"]
pub struct PhyPendingTracking {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
    transition: PhyParamTrackingTransition,
}

impl fmt::Debug for PhyPendingTracking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyPendingTracking")
            .field("request", &self.request)
            .field("action", &self.transition.action())
            .finish_non_exhaustive()
    }
}

impl PhyPendingTracking {
    /// Whether this request's registration still describes `hardware`.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn describes(
        &self,
        hardware: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> bool {
        self.owner.describes(hardware)
    }

    /// Ordinary model fixture; no registration or physical admission proof.
    #[cfg(feature = "validation-probes")]
    pub(crate) fn for_validation(
        request: PhyParamTrackRequest,
        policy: PhyParamTrackingPolicy,
    ) -> Self {
        let mut owner = PhyClientState::without_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS);
        owner.bits = if request.wifi() { WIFI_BIT } else { 0 }
            | if request.bluetooth_ieee802154() {
                BLUETOOTH_BIT
            } else {
                0
            };
        owner.tracker_model_armed = owner.bits != 0;
        Self {
            owner,
            request,
            transition: PhyParamTrackingTransition::new(request, policy),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(request: PhyParamTrackRequest, policy: PhyParamTrackingPolicy) -> Self {
        Self {
            owner: PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS),
            request,
            transition: PhyParamTrackingTransition::new(request, policy),
        }
    }

    pub(crate) const fn action(&self) -> PhyParamTrackingAction {
        self.transition.action()
    }

    pub(crate) fn advance(
        &mut self,
        completion: PhyParamTrackingCompletion,
    ) -> Result<(), PhyParamTrackingTransitionError> {
        self.transition.advance(completion)
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }

    /// Lower the current RFPLL-cap action into its complete typed child.
    pub(crate) fn begin_rfpll_cap_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingRfpllTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_rfpll_cap_tracking(state)
    }

    /// Lower the current calibration action while retaining all three live
    /// semantic temperature references until terminal commit.
    pub(crate) fn begin_calibration_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingCalibrationTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_calibration_tracking(state)
    }

    /// Lower the current outer TX-power action into its complete typed child.
    /// Every other outer action fails closed instead of becoming a no-op.
    pub(crate) fn begin_tx_power_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingTxPowerTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_tx_power_tracking(state)
    }

    /// Lower the current Wi-Fi PHY-I2C action into its complete typed child.
    pub(crate) fn begin_wifi_i2c_tracking<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingWifiI2cTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_wifi_i2c_tracking(state)
    }

    /// Lower the current final sensor action into its complete typed child.
    pub(crate) fn begin_temperature_read<'state>(
        &self,
        state: &'state mut crate::state::PhyState,
    ) -> Result<PhyParamTrackingTemperatureTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_temperature_read(state)
    }

    /// Recover the ordinary owner only after every outer action was confirmed.
    pub fn into_owner(self) -> Result<PhyClientState, Self> {
        if matches!(
            self.transition.action(),
            PhyParamTrackingAction::Complete(_)
        ) {
            Ok(self.owner)
        } else {
            Err(self)
        }
    }

    /// Poison an interrupted or failed hardware tracking operation.
    pub fn fail(self) -> PhyTrackPoisoned {
        PhyTrackPoisoned {
            owner: self.owner,
            request: self.request,
        }
    }
}

/// Terminal model owner after tracking hardware failed or was cancelled.
///
/// No ordinary-owner extractor is provided because the model has no reviewed
/// rollback for partially executed hardware work.
#[must_use = "failed tracking hardware work poisons the PHY client model"]
pub struct PhyTrackPoisoned {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
}

impl PhyTrackPoisoned {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        &self.request
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }
}

/// Fail-closed monotonic-clock validation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTrackTimeError {
    DeadlineOverflow {
        class: PhyPllTrackClass,
    },
    TimeReversed {
        class: PhyPllTrackClass,
        previous_micros: u64,
        now_micros: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientAcquireError {
    AlreadyAcquired(RadioClient),
    TrackingTime(PhyTrackTimeError),
}

/// Failed acquisition retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyClientAcquireFailure {
    owner: PhyClientState,
    error: PhyClientAcquireError,
}

impl fmt::Debug for PhyClientAcquireFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientAcquireFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyClientAcquireFailure {
    pub const fn error(&self) -> PhyClientAcquireError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Successful pure acquisition and its source-reviewed facts.
#[must_use = "the outcome retains the unique PHY client owner"]
pub struct PhyClientAcquireOutcome {
    client: RadioClient,
    was_empty: bool,
    ordering: PhyClientAcquireOrdering,
    continuation: TrackContinuation,
}

impl fmt::Debug for PhyClientAcquireOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientAcquireOutcome")
            .field("client", &self.client)
            .field("was_empty", &self.was_empty)
            .field("ordering", &self.ordering)
            .field("track_requested", &self.request().is_some())
            .finish_non_exhaustive()
    }
}

impl PhyClientAcquireOutcome {
    pub const fn client(&self) -> RadioClient {
        self.client
    }

    pub const fn was_empty(&self) -> bool {
        self.was_empty
    }

    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.ordering
    }

    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.continuation.request()
    }

    pub const fn owner(&self) -> &PhyClientState {
        self.continuation.owner()
    }

    /// Recover the ordinary owner only when no tracking request is pending.
    pub fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        self.continuation.into_owner()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientReleaseError {
    NotAcquired(RadioClient),
}

/// Failed release retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyClientReleaseFailure {
    owner: PhyClientState,
    error: PhyClientReleaseError,
}

impl fmt::Debug for PhyClientReleaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyClientReleaseFailure {
    pub const fn error(&self) -> PhyClientReleaseError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Successful pure release and its saved-mask last-user fact.
#[must_use = "the outcome retains the unique PHY client owner"]
pub struct PhyClientReleaseOutcome {
    owner: PhyClientState,
    client: RadioClient,
    is_last: bool,
}

impl fmt::Debug for PhyClientReleaseOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientReleaseOutcome")
            .field("client", &self.client)
            .field("is_last", &self.is_last)
            .finish_non_exhaustive()
    }
}

impl PhyClientReleaseOutcome {
    pub const fn client(&self) -> RadioClient {
        self.client
    }

    pub const fn is_last(&self) -> bool {
        self.is_last
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Failed scheduler evaluation retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyTrackEvaluationFailure {
    owner: PhyClientState,
    error: PhyTrackTimeError,
}

impl fmt::Debug for PhyTrackEvaluationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyTrackEvaluationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyTrackEvaluationFailure {
    pub const fn error(&self) -> PhyTrackTimeError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Pure scheduler result retaining the unique software owner.
#[must_use = "the evaluation retains the unique PHY client owner"]
pub struct PhyTrackEvaluation {
    continuation: TrackContinuation,
}

impl fmt::Debug for PhyTrackEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyTrackEvaluation")
            .field("track_requested", &self.request().is_some())
            .finish_non_exhaustive()
    }
}

impl PhyTrackEvaluation {
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.continuation.request()
    }

    pub const fn owner(&self) -> &PhyClientState {
        self.continuation.owner()
    }

    /// Recover the ordinary owner only when no tracking request is pending.
    pub fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        self.continuation.into_owner()
    }
}

#[cfg(test)]
mod tests;
