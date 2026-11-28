//! The owner's operations on the target: registration, close, wake, the
//! channel and a tracking pass, each over the place of the PHY's state and
//! the proof of RF its life is in.
//!
//! Each operation takes the radio domain by value and gives it back.

use oer_chips_esp32s31_hal::power::PlatformClockBackend;
use oer_chips_esp32s31_hal::radio::domain::transaction::Operation;
use oer_chips_esp32s31_hal::radio::domain::{
    ModemClockError, PhyClockModule, PlatformClockProvider, RadioDomain,
};

use super::domain::{PhyRegisterFailed, register_state};
use super::*;
use crate::PhyConfig;
use crate::executor::boundary::Boundary;
use crate::owner::{PhyPlace, RegisteredPhy, RfClosed, RfOpen, Vacate};

/// A completed registration: the registered place, RF open, and what the
/// registration produced.
#[must_use = "a registration's place and its RF proof are the owner's"]
pub struct Registered<'p> {
    /// The registered place.
    pub phy: RegisteredPhy<'p>,
    /// RF is open.
    pub open: RfOpen,
    /// The calibration cache the registration made, when it made one.
    pub calibration_cache: Option<PhyCalibrationCache>,
    /// The registration's outcome.
    pub outcome: PhyRegisterOutcome,
    /// The port's counters.
    pub counters: PhyTargetPortCounters,
}

/// A registration that did not complete: its place, empty, and the leaf
/// cause. A failure after the place was filled empties it.
#[must_use = "the empty place is the owner's for the next registration"]
pub struct RegistrationFailure<'p> {
    /// The place, empty.
    pub place: PhyPlace<'p>,
    /// Why.
    pub cause: RegistrationCause,
}

/// Why a registration did not complete.
#[derive(Clone, Copy, Debug)]
pub enum RegistrationCause {
    /// `PHY` or `PHY_CALIBRATION` was refused before any register access.
    Clock(ModemClockError),
    /// The registration failed; `clocks` is the release of both modules
    /// after it.
    Failed {
        /// The registration's failure.
        failure: PhyRegisterFailed,
        /// The release of `PHY` and `PHY_CALIBRATION`.
        clocks: Result<(), ModemClockError>,
    },
    /// The registration completed and `PHY_CALIBRATION` was not released.
    CalibrationClock(ModemClockError),
}

/// A close that did not complete.
#[derive(Debug)]
pub enum CloseFailure {
    /// The preparation failed after every issued edge completed: RF is still
    /// open, and its proof comes back.
    StillOpen(RfOpen, PhyTargetPortError),
    /// An edge of the close failed: the proof is gone, and the right to
    /// empty the place is left.
    Failed(PhyTargetPortError, Vacate),
    /// RF closed and `PHY` was not released.
    Clock(ModemClockError, Vacate),
}

/// A wake that did not complete.
#[derive(Debug)]
pub enum WakeFailure {
    /// `PHY` or `PHY_CALIBRATION` was refused before any register access: RF
    /// is still closed, and its proof comes back.
    StillClosed(RfClosed, ModemClockError),
    /// An edge of the wake failed: the proof is gone, and the right to empty
    /// the place is left.
    Failed(PhyTargetPortError, Vacate),
    /// RF opened and `PHY_CALIBRATION` was not released.
    Clock(ModemClockError, Vacate),
}

/// A tracking pass that did not complete: its cause, and the right to
/// empty the place.
#[derive(Debug)]
pub struct TrackingFailure {
    /// Why.
    pub error: TargetPhyParamTrackingError,
    /// The right to empty the place.
    pub vacate: Vacate,
}

/// A channel selection or switch that did not complete: its cause, and the
/// right to empty the place.
#[derive(Debug)]
pub struct ChannelFailure {
    /// Why.
    pub error: PhyTargetPortError,
    /// The right to empty the place.
    pub vacate: Vacate,
}

/// Take `PHY`, then `PHY_CALIBRATION`, as `esp_phy_enable` does before
/// calibration or RF wake.
fn enable_phy_clocks<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B>,
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
    radio: &mut RadioDomain<B>,
) -> Result<(), ModemClockError> {
    let calibration = radio.disable_phy_modem_clocks(PhyClockModule::Calibration);
    let phy = radio.disable_phy_modem_clocks(PhyClockModule::Phy);
    calibration.and(phy)
}

/// Register the PHY into `place`: the calibrating first `esp_phy_enable`.
///
/// `PHY` and `PHY_CALIBRATION` are enabled, the place is filled with a new
/// state and the registration runs over it; `PHY_CALIBRATION` is released
/// after it and `PHY` stays held while RF is open. A failure empties the
/// place and gives it back. A future let go in the middle takes the place
/// with it, as it already takes the radio.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
#[must_use = "PHY registration must be driven to a terminal result"]
pub fn register<'a, 'p: 'a, B: PlatformClockBackend + 'a, P, D, O>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    radio: RadioDomain<B>,
    place: PhyPlace<'p>,
    platform: &'a mut P,
    clocks: &'a impl PlatformClockProvider<Backend = B>,
    config: PhyRegisterConfig,
    observer: O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B>,
        Result<Registered<'p>, RegistrationFailure<'p>>,
    ),
> + 'a
where
    D: PhyShortDelay + 'a,
    O: PhyTargetObserver + 'a,
{
    let initial = radio.detach(Operation::Register);
    Boundary::new(async move {
        let mut radio = initial.reunite();
        if let Err(error) = enable_phy_clocks(&mut radio, clocks) {
            return (
                radio,
                Err(RegistrationFailure {
                    place,
                    cause: RegistrationCause::Clock(error),
                }),
            );
        }
        let mut filled = place.fill(PhyState::new(PhyConfig::esp32s31_default()));
        let mut transaction = radio.detach(Operation::Register);
        let result = register_state::<P, _, D, O>(
            timer,
            platform,
            transaction.hardware_mut(),
            &mut filled.state,
            config,
            observer,
        )
        .await;
        let mut radio = transaction.reunite();
        match result {
            Ok(registration) => match radio.disable_phy_modem_clocks(PhyClockModule::Calibration) {
                Ok(()) => (
                    radio,
                    Ok(Registered {
                        phy: RegisteredPhy::registered(filled, registration.witness),
                        open: RfOpen::opened(),
                        calibration_cache: registration.calibration_cache,
                        outcome: registration.outcome,
                        counters: registration.counters,
                    }),
                ),
                Err(error) => (
                    radio,
                    Err(RegistrationFailure {
                        place: filled.empty(),
                        cause: RegistrationCause::CalibrationClock(error),
                    }),
                ),
            },
            Err(failure) => {
                let clocks = release_phy_clocks(&mut radio);
                (
                    radio,
                    Err(RegistrationFailure {
                        place: filled.empty(),
                        cause: RegistrationCause::Failed { failure, clocks },
                    }),
                )
            }
        }
    })
}

/// Close RF: the last `esp_phy_disable`. The temperature preflight and
/// `phy_close_rf` run, the temperature sensor powers down (`phy_xpd_tsens`)
/// and `PHY` is released. The registration stays, for [`wake`].
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn close<'a, B: PlatformClockBackend + 'a, P, D>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    radio: RadioDomain<B>,
    phy: &'a mut RegisteredPhy<'_>,
    open: RfOpen,
    platform: &'a mut P,
) -> impl core::future::Future<Output = (RadioDomain<B>, Result<RfClosed, CloseFailure>)> + 'a
where
    D: PhyShortDelay + 'a,
{
    let initial = radio.detach(Operation::Close);
    Boundary::new(async move {
        let mut transaction = initial;
        let registers = transaction.hardware_mut();
        let observed = core::pin::pin!(radio_lifecycle::observe_temperature_with_hal::<P, D>(
            timer,
            platform,
            registers,
            phy.state_mut(),
        ))
        .await;
        let closed = match observed {
            Ok(()) => radio_lifecycle::execute_rf_close_with_hal::<D, _>(timer, registers)
                .and_then(|()| {
                    // SOURCE: linked IDF override releases TSENS and the last
                    // peripheral SAR-I2C reference after phy_close_rf.
                    oer_chips_esp32s31_hal::radio::phy::temperature::release(registers)
                        .map_err(|_| PhyTargetPortError::HardwareInvariant)
                })
                .map_err(PhyRfCloseTemperatureFailure::HardwareAmbiguous),
            Err(failure) => Err(failure),
        };
        let mut radio = transaction.reunite();
        let result = match closed {
            Ok(()) => match radio.disable_phy_modem_clocks(PhyClockModule::Phy) {
                Ok(()) => Ok(open.close()),
                Err(error) => Err(CloseFailure::Clock(error, open.fail())),
            },
            Err(PhyRfCloseTemperatureFailure::Recoverable(error)) => {
                Err(CloseFailure::StillOpen(open, error))
            }
            Err(PhyRfCloseTemperatureFailure::HardwareAmbiguous(error)) => {
                Err(CloseFailure::Failed(error, open.fail()))
            }
        };
        (radio, result)
    })
}

/// Wake RF from the retained registration: a later first
/// `esp_phy_enable`. `PHY` and `PHY_CALIBRATION` are enabled,
/// `phy_wakeup_init` restores the registration without calibration and
/// `PHY_CALIBRATION` is released. The owner asks [`crate::owner::tracking_due`]
/// after it.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn wake<'a, B: PlatformClockBackend + 'a, D>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    radio: RadioDomain<B>,
    phy: &'a RegisteredPhy<'_>,
    closed: RfClosed,
    clocks: &'a impl PlatformClockProvider<Backend = B>,
) -> impl core::future::Future<Output = (RadioDomain<B>, Result<RfOpen, WakeFailure>)> + 'a
where
    D: PhyShortDelay + 'a,
{
    let initial = radio.detach(Operation::Wake);
    Boundary::new(async move {
        let mut radio = initial.reunite();
        if let Err(error) = enable_phy_clocks(&mut radio, clocks) {
            return (radio, Err(WakeFailure::StillClosed(closed, error)));
        }
        let mut transaction = radio.detach(Operation::Wake);
        let woken = core::pin::pin!(radio_lifecycle::execute_rf_wake_with_hal::<D>(
            timer,
            transaction.hardware_mut(),
            phy.state(),
        ))
        .await;
        let mut radio = transaction.reunite();
        let result = match woken {
            Ok(()) => match radio.disable_phy_modem_clocks(PhyClockModule::Calibration) {
                Ok(()) => Ok(closed.open()),
                Err(error) => Err(WakeFailure::Clock(error, closed.fail())),
            },
            Err(error) => Err(WakeFailure::Failed(error, closed.fail())),
        };
        (radio, result)
    })
}

/// Select the Wi-Fi channel: `phy_chip_set_chan`.
///
/// # Cancellation
///
/// Dropping the future, including before its first poll, quarantines the
/// transaction and records its cancelled channel operation.
pub async fn select_channel<
    'a,
    D: PhyShortDelay,
    B: PlatformClockBackend,
    P,
    O: PhyTargetObserver,
>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    channel_or_frequency: u16,
    cbw: u8,
    mut transaction: oer_chips_esp32s31_hal::radio::ieee80211::channel::ChannelTransaction<
        'a,
        B,
        P,
    >,
    phy: &'a mut RegisteredPhy<'_>,
    open: RfOpen,
    observer: &'a mut O,
) -> (RadioDomain<B>, Result<RfOpen, ChannelFailure>) {
    let result = {
        let mut channel = transaction.channel_mut();
        select_phy_channel_with_hal::<D, _, _>(
            timer,
            phy.state_mut(),
            channel_or_frequency,
            cbw,
            &mut channel,
            observer,
        )
        .await
    };
    let result = match result {
        Ok(()) => Ok(open),
        Err(error) => Err(ChannelFailure {
            error,
            vacate: open.fail(),
        }),
    };
    (transaction.reunite(), result)
}

/// Switch the Wi-Fi channel with the MAC's restart around it.
///
/// # Cancellation
///
/// Dropping the future, including before its first poll, quarantines the
/// transaction and records its cancelled channel operation.
pub async fn switch_channel<
    'a,
    D: PhyShortDelay,
    B: PlatformClockBackend,
    P,
    O: PhyTargetObserver,
>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + crate::executor::wait::WaitTally
        ),
    channel_or_frequency: u16,
    cbw: u8,
    mut transaction: oer_chips_esp32s31_hal::radio::ieee80211::channel::ChannelTransaction<
        'a,
        B,
        P,
    >,
    phy: &'a mut RegisteredPhy<'_>,
    open: RfOpen,
    observer: &'a mut O,
) -> (RadioDomain<B>, Result<RfOpen, ChannelFailure>) {
    let result = {
        let mut channel = transaction.channel_mut();
        switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
            timer,
            phy.state_mut(),
            channel_or_frequency,
            cbw,
            &mut channel,
            observer,
        )
        .await
    };
    let result = match result {
        Ok(()) => Ok(open),
        Err(error) => Err(ChannelFailure {
            error,
            vacate: open.fail(),
        }),
    };
    (transaction.reunite(), result)
}

/// Run one tracking pass with the protocols running, as the vendor's
/// `phy_param_track_tot` does under its PHY lock: the owned transaction
/// serializes the radio domain, and the tracking graph brackets its
/// RF-sensitive regions with the grant-protect request. The pass's instant
/// is the state's last tracking.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn track<'a, B: PlatformClockBackend + 'a, P, D, O>(
    timer: &'a (
            impl oer_portable_cooperate::CooperativeTimer
            + oer_portable_settle::SettleCounter
            + oer_portable_clock::Clock
            + crate::executor::wait::WaitTally
        ),
    radio: RadioDomain<B>,
    phy: &'a mut RegisteredPhy<'_>,
    open: RfOpen,
    platform: &'a mut P,
    observer: O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B>,
        Result<(RfOpen, PhyParamTrackingOutcome), TrackingFailure>,
    ),
> + 'a
where
    D: PhyShortDelay + 'a,
    O: PhyTargetObserver + 'a,
{
    let mut transaction = radio.detach(Operation::Maintain);
    async move {
        let mut tracking = PhyParamTrackingTransition::new(
            crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(phy.state()),
        );
        let result = {
            let mut port = TargetPhyParamTrackingPort::<_, D, _, _>::new(
                platform,
                transaction.phy_mut(),
                timer,
                observer,
            );
            core::pin::pin!(run_phy_param_tracking(
                &mut tracking,
                phy.state_mut(),
                &mut port
            ))
            .await
            .map_err(TargetPhyParamTrackingError::Run)
        };
        let radio = transaction.reunite();
        let result = match result {
            Ok(outcome) => {
                phy.state_mut()
                    .mark_tracked(oer_portable_clock::Clock::now(timer));
                Ok((open, outcome))
            }
            Err(error) => Err(TrackingFailure {
                error,
                vacate: open.fail(),
            }),
        };
        (radio, result)
    }
}

/// Enable the Wi-Fi baseband receive path: the vendor `phy_wifi_enable_set`
/// with `true`, which ESP-IDF's `enable_phy_with_wifi_rx` runs after the PHY
/// is enabled.
pub fn enable_wifi_rx<B: PlatformClockBackend>(radio: &mut RadioDomain<B>, _open: &RfOpen) {
    radio.enable_wifi_rx();
}

/// Disable the Wi-Fi baseband receive path: `phy_wifi_enable_set` with
/// `false`, before Wi-Fi leaves the PHY.
pub fn disable_wifi_rx<B: PlatformClockBackend>(radio: &mut RadioDomain<B>, _open: &RfOpen) {
    radio.disable_wifi_rx();
}
