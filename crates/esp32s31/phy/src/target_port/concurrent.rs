//! Target registration and tracking of the shared PHY domain under the radio
//! arbiter.

use super::*;
use crate::{
    concurrent::{
        ConcurrentPhy, ConcurrentPhyError, Slot, admit_maintenance, evaluate_periodic_tracking,
    },
    domain::PhyDomain,
    state::client::RadioClient,
};
use oer_esp32s31_hal::shared_radio::{
    ModemClockError, PhyClockModule, PlatformClockProvider, SharedRadioLease,
};

/// Outputs of the shared domain's registration.
#[must_use = "the registration cache and outcome describe the new epoch"]
pub struct ConcurrentPhyRegistration {
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

impl ConcurrentPhyRegistration {
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    pub fn into_calibration_cache(self) -> Option<PhyCalibrationCache> {
        self.calibration_cache
    }
}

/// Shared domain registration that did not complete cleanly.
#[must_use = "a started registration failure is fail-stop"]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the exact registration transition"
)]
pub enum ConcurrentPhyRegisterFailure {
    /// Rejected before any register access; no modem clock is held.
    Rejected(ConcurrentPhyError),
    /// The registration transition failed and the domain stays unregistered.
    /// `clocks` reports whether the domain's modem clock modules were
    /// released.
    Failed {
        /// The failed registration transition.
        failure: PhyDomainRegisterFailure,
        /// Release of `PHY` and `PHY_CALIBRATION` after the failure.
        clocks: Result<(), ModemClockError>,
    },
    /// The domain registered, but `PHY_CALIBRATION` was not released; the
    /// modem clocks are poisoned until reset.
    CalibrationClock {
        /// The completed registration outputs.
        registration: ConcurrentPhyRegistration,
        /// Why `PHY_CALIBRATION` was not released.
        error: ModemClockError,
    },
}

/// RF close or wake of the shared domain that did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentRfError {
    /// Rejected before any register access; the domain is unchanged.
    Rejected(ConcurrentPhyError),
    /// RF close preparation failed after every issued transaction completed;
    /// RF stays open and the domain is unchanged.
    Recoverable(PhyTargetPortError),
    /// Hardware work failed ambiguously; the domain is poisoned.
    Failed(PhyTargetPortError),
    /// RF changed state, but the domain's modem clock module did not follow;
    /// the modem clocks are poisoned until reset.
    Clock(ModemClockError),
}

/// Take `PHY`, then `PHY_CALIBRATION`, as `esp_phy_enable` does before
/// calibration or RF wake.
fn enable_phy_clocks(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider,
) -> Result<(), ModemClockError> {
    lease.enable_phy_modem_clocks(PhyClockModule::Phy, clocks)?;
    if let Err(error) = lease.enable_phy_modem_clocks(PhyClockModule::Calibration, clocks) {
        return Err(match lease.disable_phy_modem_clocks(PhyClockModule::Phy) {
            Ok(()) => error,
            Err(rollback) => rollback,
        });
    }
    Ok(())
}

/// Release both modules after a registration that did not complete.
fn release_phy_clocks(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
) -> Result<(), ModemClockError> {
    let calibration = lease.disable_phy_modem_clocks(PhyClockModule::Calibration);
    let phy = lease.disable_phy_modem_clocks(PhyClockModule::Phy);
    calibration.and(phy)
}

/// Shared domain tracking that did not settle the domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentPhyTrackingError {
    /// Rejected before any register access; the domain stays pending.
    Rejected(ConcurrentPhyError),
    /// Tracking started and failed; the domain is poisoned.
    Failed(TargetPhyParamTrackingError),
}

/// Register the shared PHY domain through the arbiter's shared-PHY borrow.
///
/// This is the calibrating first `esp_phy_enable`: `PHY` and
/// `PHY_CALIBRATION` are enabled through the arbiter before calibration, and
/// `PHY_CALIBRATION` is released after it. `PHY` stays held while RF is open.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation may
/// strand a partially applied edge and leaves the domain unregistered; the
/// radio then requires reset.
#[must_use = "PHY registration must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the allocation-free PHY transition"
)]
pub async fn register_concurrent_phy<P, D, O>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &impl PlatformClockProvider,
    config: PhyRegisterConfig,
    observer: O,
) -> Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    match lease.attachment_mut().slot_mut() {
        Slot::Empty => {}
        Slot::Poisoned => {
            return Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::Poisoned,
            ));
        }
        Slot::Registered(_) | Slot::RfClosed(_) | Slot::Pending { .. } => {
            return Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::AlreadyRegistered,
            ));
        }
    }
    enable_phy_clocks(lease, clocks).map_err(|error| {
        ConcurrentPhyRegisterFailure::Rejected(ConcurrentPhyError::Clock(error))
    })?;
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    match PhyDomain::register::<P, _, D, O>(timer, platform, &mut registers, config, observer).await
    {
        Ok(registered) => {
            let (domain, calibration_cache, outcome, counters) = registered.into_parts();
            *phy.slot_mut() = Slot::Registered(domain);
            let registration = ConcurrentPhyRegistration {
                calibration_cache,
                outcome,
                counters,
            };
            match lease.disable_phy_modem_clocks(PhyClockModule::Calibration) {
                Ok(()) => Ok(registration),
                Err(error) => Err(ConcurrentPhyRegisterFailure::CalibrationClock {
                    registration,
                    error,
                }),
            }
        }
        Err(failure) => Err(ConcurrentPhyRegisterFailure::Failed {
            failure,
            clocks: release_phy_clocks(lease),
        }),
    }
}

/// Close RF after the last client left the shared domain, and release its
/// `PHY` modem clock module.
///
/// This is the last `esp_phy_disable`: the temperature preflight and
/// `phy_close_rf` run as on every route, the temperature sensor powers down
/// (`phy_xpd_tsens`), then the common PHY clock is released. The registered calibration epoch is retained for
/// [`wake_concurrent_rf`].
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-open,
/// clients are active or the borrow describes another registration. A
/// preparation failure after completed transactions keeps RF open; an
/// ambiguous hardware failure poisons the domain.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation leaves
/// the domain unregistered in software while RF may be partially closed; the
/// radio then requires reset.
pub async fn close_concurrent_rf<P, D>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
) -> Result<(), ConcurrentRfError>
where
    D: PhyShortDelay,
{
    let mut domain = lease
        .attachment_mut()
        .idle_domain()
        .map_err(ConcurrentRfError::Rejected)?;
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if !domain.clients.describes(&registers) {
        *phy.slot_mut() = Slot::Registered(domain);
        return Err(ConcurrentRfError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    let closed = match radio_lifecycle::observe_temperature_with_hal::<P, D>(
        timer,
        platform,
        &mut registers,
        domain.registered.target_state_mut(),
    )
    .await
    {
        Ok(()) => radio_lifecycle::execute_rf_close_with_hal::<D>(&mut registers)
            .map(|()| {
                // `esp_phy_disable` powers the temperature sensor down after
                // `phy_close_rf` (`phy_xpd_tsens`); the retained wake powers it
                // up again. Its SAR2 analog block is then reset like every
                // other powered-down analog block.
                oer_esp32s31_hal::phy::temperature::power_down(&mut registers);
            })
            .map_err(PhyRfCloseTemperatureFailure::HardwareAmbiguous),
        Err(failure) => Err(failure),
    };
    match closed {
        Ok(()) => {
            *phy.slot_mut() = Slot::RfClosed(domain);
            lease
                .disable_phy_modem_clocks(PhyClockModule::Phy)
                .map_err(ConcurrentRfError::Clock)
        }
        Err(PhyRfCloseTemperatureFailure::Recoverable(error)) => {
            *phy.slot_mut() = Slot::Registered(domain);
            Err(ConcurrentRfError::Recoverable(error))
        }
        Err(PhyRfCloseTemperatureFailure::HardwareAmbiguous(error)) => {
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentRfError::Failed(error))
        }
    }
}

/// Wake RF of the closed shared domain for its next client.
///
/// This is a later first `esp_phy_enable`: `PHY` and `PHY_CALIBRATION` are
/// enabled, the retained registration is restored by `phy_wakeup_init`
/// without calibration, and `PHY_CALIBRATION` is released. No client is
/// acquired; the next `acquire_client`
/// evaluates tracking.
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-closed, the
/// borrow describes another registration or the clocks could not be
/// enabled. A started wake that fails poisons the domain.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. After the first wake
/// edge every failure is fail-stop and requires reset.
pub async fn wake_concurrent_rf<D>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider,
) -> Result<(), ConcurrentRfError>
where
    D: PhyShortDelay,
{
    let domain = lease
        .attachment_mut()
        .closed_domain()
        .map_err(ConcurrentRfError::Rejected)?;
    let describes = {
        let (registers, _) = lease.phy_hal_with_attachment();
        domain.clients.describes(&registers)
    };
    if !describes {
        *lease.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return Err(ConcurrentRfError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    if let Err(error) = enable_phy_clocks(lease, clocks) {
        *lease.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return Err(ConcurrentRfError::Rejected(ConcurrentPhyError::Clock(
            error,
        )));
    }
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if let Err(error) =
        radio_lifecycle::execute_rf_wake_with_hal::<D>(timer, &mut registers, domain.phy_state())
            .await
    {
        *phy.slot_mut() = Slot::Poisoned;
        return Err(ConcurrentRfError::Failed(error));
    }
    *phy.slot_mut() = Slot::Registered(domain);
    lease
        .disable_phy_modem_clocks(PhyClockModule::Calibration)
        .map_err(ConcurrentRfError::Clock)
}

/// Run the pending tracking request with the protocols running, as the
/// vendor's tracking does under its PHY lock: the lease is that lock, and the
/// tracking graph brackets its RF-sensitive regions with the grant-protect
/// request.
///
/// A domain without pending tracking is rejected before any register
/// access.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation after
/// admission leaves the domain pending in software while hardware may be
/// partially updated; the radio then requires reset.
pub async fn maintain_concurrent_phy<P, D, O>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    observer: O,
) -> Result<PhyParamTrackingOutcome, ConcurrentPhyTrackingError>
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    admit_maintenance(lease).map_err(ConcurrentPhyTrackingError::Rejected)?;

    let (mut registered, pending) = match core::mem::take(lease.attachment_mut().slot_mut()) {
        Slot::Pending {
            registered,
            pending,
        } => (registered, pending),
        other => {
            // Admission accepted only a pending slot.
            *lease.attachment_mut().slot_mut() = other;
            return Err(ConcurrentPhyTrackingError::Rejected(
                ConcurrentPhyError::NoTrackingPending,
            ));
        }
    };
    let mut tracking = pending.begin_tracking(registered.tracking_policy());

    let (mut registers, phy, mut grant) = lease.phy_hal_with_attachment_and_grant();
    if !tracking.describes(&registers) {
        let error = TargetPhyParamTrackingError::EpochMismatch;
        *phy.slot_mut() = Slot::Poisoned;
        return Err(ConcurrentPhyTrackingError::Failed(error));
    }
    let result = {
        let state = registered.target_state_mut();
        let mut port = TargetPhyParamTrackingPort::<_, _, _, D, _, _>::new(
            platform,
            &mut registers,
            &mut grant,
            timer,
            observer,
        );
        run_phy_param_tracking(&mut tracking, state, &mut port)
            .await
            .map_err(TargetPhyParamTrackingError::Run)
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            *phy.slot_mut() = Slot::Poisoned;
            return Err(ConcurrentPhyTrackingError::Failed(error));
        }
    };
    match tracking.into_owner() {
        Ok(clients) => {
            *phy.slot_mut() = Slot::Registered(PhyDomain::new(registered, clients));
            Ok(outcome)
        }
        Err(_) => {
            let error = TargetPhyParamTrackingError::MissingCompletedOwner;
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentPhyTrackingError::Failed(error))
        }
    }
}

/// Result of one periodic tracking tick of the shared domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentTrackingTick {
    /// No tracking was due.
    NotDue,
    /// Tracking was due and ran to completion.
    Tracked(PhyParamTrackingOutcome),
    /// The domain cannot track now (not registered, RF closed, or poisoned);
    /// nothing changed.
    Unavailable(ConcurrentPhyError),
}

/// One tick of ESP-IDF's periodic `phy_track_pll`: evaluate the clients'
/// tracking period and, when tracking is due, run it at once with the
/// protocols running, as the vendor timer callback does under its PHY lock;
/// the lease is that lock, and the tracking graph brackets its RF-sensitive
/// regions with the grant-protect request.
///
/// # Errors
///
/// The tracking clock was rejected, or tracking started and failed; after a
/// failure the domain is poisoned.
///
/// # Cancellation
///
/// Once tracking starts, drive this future to a terminal result;
/// cancellation then leaves hardware partially updated and requires reset.
pub async fn track_concurrent_phy<P, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    timer: &impl oer_time::Timer,
    observer: O,
) -> Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    let due = if lease.attachment().tracking_pending() {
        true
    } else {
        match evaluate_periodic_tracking(lease, timer) {
            Ok(due) => due,
            Err(
                error @ (ConcurrentPhyError::NotRegistered
                | ConcurrentPhyError::RfClosed
                | ConcurrentPhyError::Poisoned),
            ) => return Ok(ConcurrentTrackingTick::Unavailable(error)),
            Err(error) => return Err(ConcurrentPhyTrackingError::Rejected(error)),
        }
    };
    if !due {
        return Ok(ConcurrentTrackingTick::NotDue);
    }
    maintain_concurrent_phy::<P, D, O>(timer, lease, platform, observer)
        .await
        .map(ConcurrentTrackingTick::Tracked)
}

/// Wi-Fi channel operation on the shared domain that did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentWifiChannelError {
    /// Rejected before any register access: the domain is not settled, the
    /// borrow describes another registration, or Wi-Fi is not a client.
    Rejected(ConcurrentPhyError),
    /// The channel transaction failed after it started.
    Failed(PhyTargetPortError),
}

fn wifi_channel_state<'domain>(
    phy: &'domain mut ConcurrentPhy,
    channel: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<&'domain mut PhyState, ConcurrentWifiChannelError> {
    let domain = phy
        .settled_mut()
        .map_err(ConcurrentWifiChannelError::Rejected)?;
    if !domain.clients.describes(channel) {
        return Err(ConcurrentWifiChannelError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    if !domain.client_snapshot().contains(RadioClient::Wifi) {
        return Err(ConcurrentWifiChannelError::Rejected(
            ConcurrentPhyError::ClientAbsent(RadioClient::Wifi),
        ));
    }
    Ok(domain.registered.target_state_mut())
}

/// Select a Wi-Fi channel on the shared domain before the MAC runs, as the
/// registered Wi-Fi route's channel selection does.
///
/// `phy` and `channel` come from one lease: the HAL Wi-Fi client lends the
/// channel HAL together with the arbiter attachment.
///
/// # Errors
///
/// Rejected before any register access, or failed after the channel
/// transaction started.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result.
pub async fn select_concurrent_wifi_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    phy: &mut ConcurrentPhy,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let state = wifi_channel_state(phy, channel)?;
    select_phy_channel_with_hal::<D, _, _>(
        timer,
        state,
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
    .map_err(ConcurrentWifiChannelError::Failed)
}

/// Stop the Wi-Fi MAC, retune the shared domain and restart the MAC, as the
/// registered Wi-Fi route's channel switch does.
///
/// # Errors
///
/// As [`select_concurrent_wifi_channel`].
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result.
pub async fn switch_concurrent_wifi_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    phy: &mut ConcurrentPhy,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let state = wifi_channel_state(phy, channel)?;
    switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
        timer,
        state,
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
    .map_err(ConcurrentWifiChannelError::Failed)
}
