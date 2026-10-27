//! Target registration and tracking of the shared PHY domain in the radio
//! domain.

use oer_esp32s31_hal::domain::transaction::RadioTransaction;
use oer_esp32s31_hal::power::PlatformClockBackend;
pub(super) mod operations;
pub use operations::{
    close_concurrent_rf, maintain_concurrent_phy, register_concurrent_phy, wake_concurrent_rf,
};

use super::*;
use crate::{
    concurrent::{
        ConcurrentPhy, ConcurrentPhyError, Slot, admit_maintenance, evaluate_periodic_tracking,
    },
    domain::PhyDomain,
    state::client::RadioClient,
};
use oer_esp32s31_hal::domain::{
    ModemClockError, PhyClockModule, PlatformClockProvider, RadioDomain,
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
fn enable_phy_clocks<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider<Backend = B>,
) -> Result<(), ModemClockError> {
    radio.enable_phy_modem_clocks(PhyClockModule::Phy, clocks)?;
    if let Err(error) = radio.enable_phy_modem_clocks(PhyClockModule::Calibration, clocks) {
        return Err(match radio.disable_phy_modem_clocks(PhyClockModule::Phy) {
            Ok(()) => error,
            Err(rollback) => rollback,
        });
    }
    Ok(())
}

/// Release both modules after a registration that did not complete.
fn release_phy_clocks<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B, ConcurrentPhy>,
) -> Result<(), ModemClockError> {
    let calibration = radio.disable_phy_modem_clocks(PhyClockModule::Calibration);
    let phy = radio.disable_phy_modem_clocks(PhyClockModule::Phy);
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

impl oer_diagnostics::DiagnosticPath for ConcurrentPhyTrackingError {
    fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
        match self {
            Self::Rejected(error) => {
                path.name("Rejected");
                oer_diagnostics::DiagnosticPath::push_path(error, path);
            }
            Self::Failed(error) => {
                path.name("Failed");
                oer_diagnostics::DiagnosticPath::push_path(error, path);
            }
        }
    }
}

/// Register the shared PHY domain through an owned PHY transaction.
///
/// This is the calibrating first `esp_phy_enable`: `PHY` and
/// `PHY_CALIBRATION` are enabled through the radio domain before calibration, and
/// `PHY_CALIBRATION` is released after it. `PHY` stays held while RF is open.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
#[must_use = "PHY registration must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the allocation-free PHY transition"
)]
async fn execute_register<'a, B: PlatformClockBackend + 'a, P, D, O>(
    timer: &'a impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    platform: &'a mut P,
    clocks: &'a impl PlatformClockProvider<Backend = B>,
    config: PhyRegisterConfig,
    observer: O,
) -> (
    RadioDomain<B, ConcurrentPhy>,
    Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>,
)
where
    D: PhyShortDelay + 'a,
    O: PhyTargetObserver + 'a,
{
    let mut radio = initial.reunite();
    let rejection = match radio.attachment_mut().slot_mut() {
        Slot::Empty => None,
        Slot::Poisoned => Some(ConcurrentPhyError::Poisoned),
        Slot::Registered(_) | Slot::RfClosed(_) | Slot::Pending { .. } => {
            Some(ConcurrentPhyError::AlreadyRegistered)
        }
    };
    if let Some(error) = rejection {
        return (radio, Err(ConcurrentPhyRegisterFailure::Rejected(error)));
    }
    if let Err(error) = enable_phy_clocks(&mut radio, clocks) {
        return (
            radio,
            Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::Clock(error),
            )),
        );
    }
    let mut transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Register);
    let registered = PhyDomain::register::<P, _, D, O>(
        timer,
        platform,
        transaction.hardware_mut(),
        config,
        observer,
    )
    .await;
    let mut radio = transaction.reunite();
    let result = finish_concurrent_registration(&mut radio, registered);
    (radio, result)
}

// Keep the consumed terminal result out of the registration poll's stack.
#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "failure retains the unique transition"
)]
fn finish_concurrent_registration<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B, ConcurrentPhy>,
    registered: Result<super::domain::PhyDomainRegistered, super::domain::PhyDomainRegisterFailure>,
) -> Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure> {
    match registered {
        Ok(registered) => {
            let (domain, calibration_cache, outcome, counters) = registered.into_parts();
            *radio.attachment_mut().slot_mut() = Slot::Registered(domain);
            let registration = ConcurrentPhyRegistration {
                calibration_cache,
                outcome,
                counters,
            };
            match radio.disable_phy_modem_clocks(PhyClockModule::Calibration) {
                Ok(()) => Ok(registration),
                Err(error) => Err(ConcurrentPhyRegisterFailure::CalibrationClock {
                    registration,
                    error,
                }),
            }
        }
        Err(failure) => Err(ConcurrentPhyRegisterFailure::Failed {
            failure,
            clocks: release_phy_clocks(radio),
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
/// clients are active or the owner describes another registration. A
/// preparation failure after completed transactions keeps RF open; an
/// ambiguous hardware failure poisons the domain.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
async fn execute_close<B: PlatformClockBackend, P, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    platform: &mut P,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ConcurrentRfError>)
where
    D: PhyShortDelay,
{
    let mut radio = initial.reunite();
    let mut domain = match radio.attachment_mut().idle_domain() {
        Ok(domain) => domain,
        Err(error) => return (radio, Err(ConcurrentRfError::Rejected(error))),
    };
    let mut transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Close);
    let (registers, phy) = transaction.hardware_with_attachment();
    if !domain.clients.describes(registers) {
        *phy.slot_mut() = Slot::Registered(domain);
        return (
            transaction.reunite(),
            Err(ConcurrentRfError::Rejected(
                ConcurrentPhyError::EpochMismatch,
            )),
        );
    }
    let closed = match radio_lifecycle::observe_temperature_with_hal::<P, D>(
        timer,
        platform,
        registers,
        domain.registered.target_state_mut(),
    )
    .await
    {
        Ok(()) => radio_lifecycle::execute_rf_close_with_hal::<D, _>(timer, registers)
            .and_then(|()| {
                // SOURCE: linked IDF override releases TSENS and the last
                // peripheral SAR-I2C reference after phy_close_rf.
                oer_esp32s31_hal::phy::temperature::release(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant)
            })
            .map_err(PhyRfCloseTemperatureFailure::HardwareAmbiguous),
        Err(failure) => Err(failure),
    };
    let result = match closed {
        Ok(()) => {
            *phy.slot_mut() = Slot::RfClosed(domain);
            Ok(())
        }
        Err(PhyRfCloseTemperatureFailure::Recoverable(error)) => {
            *phy.slot_mut() = Slot::Registered(domain);
            Err(ConcurrentRfError::Recoverable(error))
        }
        Err(PhyRfCloseTemperatureFailure::HardwareAmbiguous(error)) => {
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentRfError::Failed(error))
        }
    };
    let mut radio = transaction.reunite();
    let result = result.and_then(|()| {
        radio
            .disable_phy_modem_clocks(PhyClockModule::Phy)
            .map_err(ConcurrentRfError::Clock)
    });
    (radio, result)
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
/// owner describes another registration or the clocks could not be
/// enabled. A started wake that fails poisons the domain.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
async fn execute_wake<B: PlatformClockBackend, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider<Backend = B>,
) -> (RadioDomain<B, ConcurrentPhy>, Result<(), ConcurrentRfError>)
where
    D: PhyShortDelay,
{
    let mut radio = initial.reunite();
    let domain = match radio.attachment_mut().closed_domain() {
        Ok(domain) => domain,
        Err(error) => return (radio, Err(ConcurrentRfError::Rejected(error))),
    };
    let transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Wake);
    let describes = domain.clients.describes(transaction.hardware());
    let mut radio = transaction.reunite();
    if !describes {
        *radio.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return (
            radio,
            Err(ConcurrentRfError::Rejected(
                ConcurrentPhyError::EpochMismatch,
            )),
        );
    }
    if let Err(error) = enable_phy_clocks(&mut radio, clocks) {
        *radio.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return (
            radio,
            Err(ConcurrentRfError::Rejected(ConcurrentPhyError::Clock(
                error,
            ))),
        );
    }
    let mut transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Wake);
    let result = radio_lifecycle::execute_rf_wake_with_hal::<D>(
        timer,
        transaction.hardware_mut(),
        domain.phy_state(),
    )
    .await;
    let result = match result {
        Ok(()) => {
            *transaction.attachment_mut().slot_mut() = Slot::Registered(domain);
            Ok(())
        }
        Err(error) => {
            *transaction.attachment_mut().slot_mut() = Slot::Poisoned;
            Err(ConcurrentRfError::Failed(error))
        }
    };
    let mut radio = transaction.reunite();
    let result = result.and_then(|()| {
        radio
            .disable_phy_modem_clocks(PhyClockModule::Calibration)
            .map_err(ConcurrentRfError::Clock)
    });
    (radio, result)
}

/// Run the pending tracking request with the protocols running, as the
/// vendor's tracking does under its PHY lock: the owned PHY transaction serializes
/// the radio domain, and the
/// tracking graph brackets its RF-sensitive regions with the grant-protect
/// request.
///
/// A domain without pending tracking is rejected before any register
/// access.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
async fn execute_maintain<B: PlatformClockBackend, P, D, O>(
    timer: &impl oer_cooperate::CooperativeTimer,
    initial: RadioTransaction<B, ConcurrentPhy>,
    platform: &mut P,
    observer: O,
    pilot: &mut impl crate::sar_pilot::Service,
) -> (
    RadioDomain<B, ConcurrentPhy>,
    Result<PhyParamTrackingOutcome, ConcurrentPhyTrackingError>,
)
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    let mut radio = initial.reunite();
    if let Err(error) = admit_maintenance(&radio) {
        return (radio, Err(ConcurrentPhyTrackingError::Rejected(error)));
    }
    let (mut registered, pending) = match radio.attachment_mut().take_tracking() {
        Slot::Pending {
            registered,
            pending,
        } => (registered, pending),
        other => {
            *radio.attachment_mut().slot_mut() = other;
            return (
                radio,
                Err(ConcurrentPhyTrackingError::Rejected(
                    ConcurrentPhyError::NoTrackingPending,
                )),
            );
        }
    };
    let mut tracking = pending.begin_tracking(registered.tracking_policy());
    let fault = radio.fault_handle();
    let mut transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
    if !tracking.describes(transaction.hardware()) {
        *transaction.attachment_mut().slot_mut() = Slot::Poisoned;
        return (
            transaction.reunite(),
            Err(ConcurrentPhyTrackingError::Failed(
                TargetPhyParamTrackingError::EpochMismatch,
            )),
        );
    }
    let (mut returned, phy) = transaction.split();
    let mut physical = crate::sar_pilot::Physical::new(phy, pilot, fault);
    let result = {
        let state = registered.target_state_mut();
        let mut port = TargetPhyParamTrackingPort::<_, _, D, _, _>::new(
            platform,
            &mut physical,
            timer,
            observer,
        );
        run_phy_param_tracking(&mut tracking, state, &mut port)
            .await
            .map_err(TargetPhyParamTrackingError::Run)
    };
    let result = match result {
        Err(error) => {
            *returned.attachment_mut().slot_mut() = Slot::Poisoned;
            Err(ConcurrentPhyTrackingError::Failed(error))
        }
        Ok(outcome) => match tracking.into_owner() {
            Ok(clients) => {
                *returned.attachment_mut().slot_mut() =
                    Slot::Registered(PhyDomain::new(registered, clients));
                Ok(outcome)
            }
            Err(_) => {
                *returned.attachment_mut().slot_mut() = Slot::Poisoned;
                Err(ConcurrentPhyTrackingError::Failed(
                    TargetPhyParamTrackingError::MissingCompletedOwner,
                ))
            }
        },
    };
    let phy = physical.finish();
    let radio = match returned.reunite(phy) {
        Ok(radio) => radio,
        Err(refused) => {
            let held = core::mem::ManuallyDrop::new(refused.domain);
            let _quarantined = held;
            refused.transaction.quarantine_sar_transfer(
                oer_esp32s31_hal::domain::transaction::SarTransferFault::ForeignReunion {
                    expected_owner: refused.expected_owner,
                    received_owner: refused.received_owner,
                },
            );
        }
    };
    (radio, result)
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

impl oer_diagnostics::DiagnosticPath for ConcurrentTrackingTick {
    /// A completed tick names its variant and marks the path truncated: the
    /// outcome's fields are not part of a failure path.
    fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
        match self {
            Self::NotDue => path.name("NotDue"),
            Self::Tracked(_) => {
                path.name("Tracked");
                path.truncated();
            }
            Self::Unavailable(error) => {
                path.name("Unavailable");
                oer_diagnostics::DiagnosticPath::push_path(error, path);
            }
        }
    }
}

/// One tick of ESP-IDF's periodic `phy_track_pll`: evaluate the clients'
/// tracking period and, when tracking is due, run it at once with the
/// protocols running, as the vendor timer callback does under its PHY lock;
/// the owned PHY transaction serializes the radio domain, and the tracking
/// graph brackets its RF-sensitive
/// regions with the grant-protect request.
///
/// # Errors
///
/// The tracking clock was rejected, or tracking started and failed; after a
/// failure the domain is poisoned.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn track_concurrent_phy<B: PlatformClockBackend, P, D, O>(
    radio: RadioDomain<B, ConcurrentPhy>,
    platform: &mut P,
    timer: &impl oer_cooperate::CooperativeTimer,
    observer: O,
    pilot: &mut impl crate::sar_pilot::Service,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>,
    ),
>
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    let initial = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
    async move {
        let mut radio = initial.reunite();
        let due = if radio.attachment().tracking_pending() {
            true
        } else {
            match evaluate_periodic_tracking(&mut radio, timer) {
                Ok(due) => due,
                Err(
                    error @ (ConcurrentPhyError::NotRegistered
                    | ConcurrentPhyError::RfClosed
                    | ConcurrentPhyError::Poisoned),
                ) => return (radio, Ok(ConcurrentTrackingTick::Unavailable(error))),
                Err(error) => return (radio, Err(ConcurrentPhyTrackingError::Rejected(error))),
            }
        };
        if !due {
            return (radio, Ok(ConcurrentTrackingTick::NotDue));
        }
        let (radio, result) =
            maintain_concurrent_phy::<_, P, D, O>(timer, radio, platform, observer, pilot).await;
        (radio, result.map(ConcurrentTrackingTick::Tracked))
    }
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
/// The HAL Wi-Fi client consumes the owned PHY transaction into channel
/// custody, borrowing only its MAC and platform. The future owns that custody.
///
/// # Errors
///
/// Rejected before any register access, or failed after the channel
/// transaction started.
///
/// # Cancellation
///
/// Dropping the future, including before its first poll, quarantines the
/// transaction and records its cancelled channel/retune operation.
pub fn select_concurrent_wifi_channel<
    D: PhyShortDelay,
    B: PlatformClockBackend,
    P,
    O: PhyTargetObserver,
>(
    timer: &impl oer_cooperate::CooperativeTimer,
    channel_or_frequency: u16,
    cbw: u8,
    mut transaction: oer_esp32s31_hal::ieee80211::channel::ChannelTransaction<
        '_,
        B,
        P,
        ConcurrentPhy,
    >,
    observer: &mut O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<(), ConcurrentWifiChannelError>,
    ),
> {
    async move {
        let result = {
            let (mut channel, phy) = transaction.parts_mut();
            match wifi_channel_state(phy, &channel) {
                Err(error) => Err(error),
                Ok(state) => select_phy_channel_with_hal::<D, _, _>(
                    timer,
                    state,
                    channel_or_frequency,
                    cbw,
                    &mut channel,
                    observer,
                )
                .await
                .map_err(ConcurrentWifiChannelError::Failed),
            }
        };
        (transaction.reunite(), result)
    }
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
/// Dropping the future, including before its first poll, quarantines the
/// transaction and records its cancelled channel/retune operation.
pub fn switch_concurrent_wifi_channel<
    D: PhyShortDelay,
    B: PlatformClockBackend,
    P,
    O: PhyTargetObserver,
>(
    timer: &impl oer_cooperate::CooperativeTimer,
    channel_or_frequency: u16,
    cbw: u8,
    mut transaction: oer_esp32s31_hal::ieee80211::channel::ChannelTransaction<
        '_,
        B,
        P,
        ConcurrentPhy,
    >,
    observer: &mut O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<(), ConcurrentWifiChannelError>,
    ),
> {
    async move {
        let result = {
            let (mut channel, phy) = transaction.parts_mut();
            match wifi_channel_state(phy, &channel) {
                Err(error) => Err(error),
                Ok(state) => switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
                    timer,
                    state,
                    channel_or_frequency,
                    cbw,
                    &mut channel,
                    observer,
                )
                .await
                .map_err(ConcurrentWifiChannelError::Failed),
            }
        };
        (transaction.reunite(), result)
    }
}
