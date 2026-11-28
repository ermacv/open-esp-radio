//! The owner's operations on the target: registration, close, wake, the
//! channel and a tracking pass, each lent the radio domain and the handle
//! of the PHY's place, each taking the proof of RF its life is in and
//! giving the next.
//!
//! An operation is a plain `async fn`. Letting its future go ends its
//! borrows: the domain and the handle are the owner's at once, and the
//! proof it took is gone with it ([`crate::owner`]).

use oer_chips_esp32s31_hal::power::PlatformClockBackend;
use oer_chips_esp32s31_hal::radio::domain::{
    ModemClockError, PhyClockModule, PlatformClockProvider, RadioDomain,
};
use oer_chips_esp32s31_hal::radio::ieee80211::channel::RadioChannelHal;

use super::domain::{PhyRegisterFailed, register_state};
use super::*;
use crate::owner::{ChipPhy, PhyHandle, RfClosed, RfCold, RfOpen, Vacate};

/// A completed registration: RF open, and what the registration produced.
#[must_use = "RF stays open until a close takes this proof"]
pub struct Registered {
    /// RF is open.
    pub open: RfOpen<ChipPhy>,
    /// The calibration cache the registration made, when it made one.
    pub calibration_cache: Option<PhyCalibrationCache>,
    /// The registration's outcome.
    pub outcome: PhyRegisterOutcome,
    /// The port's counters.
    pub counters: PhyTargetPortCounters,
}

/// A registration that did not complete.
#[derive(Debug)]
pub enum RegistrationFailure {
    /// `PHY` or `PHY_CALIBRATION` was refused before any PHY register was
    /// written: RF is still cold, and its proof comes back.
    Clock(RfCold<ChipPhy>, ModemClockError),
    /// The registration failed; `clocks` is the release of both modules
    /// after it. The proof is gone; the right to empty the place is left.
    Failed {
        /// The registration's failure.
        failure: PhyRegisterFailed,
        /// The release of `PHY` and `PHY_CALIBRATION`.
        clocks: Result<(), ModemClockError>,
        /// The right to empty the place.
        vacate: Vacate<ChipPhy>,
    },
    /// The registration completed and `PHY_CALIBRATION` was not released.
    CalibrationClock(ModemClockError, Vacate<ChipPhy>),
}

/// A close that did not complete.
#[derive(Debug)]
pub enum CloseFailure {
    /// The temperature observation before the first RF edge failed and
    /// left TSENS as the wake set it: RF is still open, and its proof comes
    /// back.
    StillOpen(RfOpen<ChipPhy>, PhyTargetPortError),
    /// An edge of the close failed: the proof is gone, and the right to
    /// empty the place is left.
    Failed(PhyTargetPortError, Vacate<ChipPhy>),
    /// RF closed and `PHY` was not released.
    Clock(ModemClockError, Vacate<ChipPhy>),
}

/// A wake that did not complete.
#[derive(Debug)]
pub enum WakeFailure {
    /// `PHY` or `PHY_CALIBRATION` was refused before any register access: RF
    /// is still closed, and its proof comes back.
    StillClosed(RfClosed<ChipPhy>, ModemClockError),
    /// An edge of the wake failed: the proof is gone, and the right to empty
    /// the place is left.
    Failed(PhyTargetPortError, Vacate<ChipPhy>),
    /// RF opened and `PHY_CALIBRATION` was not released.
    Clock(ModemClockError, Vacate<ChipPhy>),
}

/// A tracking pass that did not complete: its cause, and the right to
/// empty the place.
#[derive(Debug)]
pub struct TrackingFailure {
    /// Why.
    pub error: TargetPhyParamTrackingError,
    /// The right to empty the place.
    pub vacate: Vacate<ChipPhy>,
}

/// A channel selection or switch that did not complete: its cause, and the
/// right to empty the place.
#[derive(Debug)]
pub struct ChannelFailure {
    /// Why.
    pub error: PhyTargetPortError,
    /// The right to empty the place.
    pub vacate: Vacate<ChipPhy>,
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

/// Register the PHY in the handle's place: the calibrating first
/// `esp_phy_enable`.
///
/// `PHY` and `PHY_CALIBRATION` are enabled; their refusal is the refusal
/// before the acceptance point, with `RfCold` back. The registration then
/// runs over the handle's state; `PHY_CALIBRATION` is released after it and
/// `PHY` stays held while RF is open.
#[must_use = "PHY registration must be driven to a terminal result"]
pub async fn register<B: PlatformClockBackend, P, D: PhyShortDelay, O: PhyTargetObserver>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    radio: &mut RadioDomain<B>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    cold: RfCold<ChipPhy>,
    platform: &mut P,
    clocks: &impl PlatformClockProvider<Backend = B>,
    config: PhyRegisterConfig,
    observer: O,
) -> Result<Registered, RegistrationFailure> {
    if let Err(error) = enable_phy_clocks(radio, clocks) {
        return Err(RegistrationFailure::Clock(cold, error));
    }
    let result = register_state::<P, _, D, O>(
        timer,
        platform,
        &mut radio.phy_hardware(),
        phy.state_mut(),
        config,
        observer,
    )
    .await;
    match result {
        Ok(registration) => match radio.disable_phy_modem_clocks(PhyClockModule::Calibration) {
            Ok(()) => {
                registration.witness.spend();
                Ok(Registered {
                    open: cold.registered(),
                    calibration_cache: registration.calibration_cache,
                    outcome: registration.outcome,
                    counters: registration.counters,
                })
            }
            Err(error) => Err(RegistrationFailure::CalibrationClock(error, cold.fail())),
        },
        Err(failure) => Err(RegistrationFailure::Failed {
            failure,
            clocks: release_phy_clocks(radio),
            vacate: cold.fail(),
        }),
    }
}

/// Close RF: the last `esp_phy_disable`. The temperature preflight and
/// `phy_close_rf` run, the temperature sensor powers down (`phy_xpd_tsens`)
/// and `PHY` is released. The registration stays, for [`wake`].
///
/// The acceptance point is the first RF edge: the temperature observation
/// before it changes nothing of what `RfOpen` says, and its recoverable
/// failure gives the proof back.
pub async fn close<B: PlatformClockBackend, P, D: PhyShortDelay>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    radio: &mut RadioDomain<B>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<ChipPhy>,
    platform: &mut P,
) -> Result<RfClosed<ChipPhy>, CloseFailure> {
    let closed = {
        let mut registers = radio.phy_hardware();
        let observed = radio_lifecycle::observe_temperature_with_hal::<P, D>(
            timer,
            platform,
            &mut registers,
            phy.state_mut(),
        )
        .await;
        match observed {
            Ok(()) => radio_lifecycle::execute_rf_close_with_hal::<D, _>(timer, &mut registers)
                .and_then(|()| {
                    // SOURCE: linked IDF override releases TSENS and the last
                    // peripheral SAR-I2C reference after phy_close_rf.
                    oer_chips_esp32s31_hal::radio::phy::temperature::release(&mut registers)
                        .map_err(|_| PhyTargetPortError::HardwareInvariant)
                })
                .map_err(PhyRfCloseTemperatureFailure::HardwareAmbiguous),
            Err(failure) => Err(failure),
        }
    };
    match closed {
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
    }
}

/// Wake RF from the retained registration: a later first
/// `esp_phy_enable`. `PHY` and `PHY_CALIBRATION` are enabled,
/// `phy_wakeup_init` restores the registration without calibration and
/// `PHY_CALIBRATION` is released. The owner asks [`crate::owner::tracking_due`]
/// after it.
pub async fn wake<B: PlatformClockBackend, D: PhyShortDelay>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    radio: &mut RadioDomain<B>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    closed: RfClosed<ChipPhy>,
    clocks: &impl PlatformClockProvider<Backend = B>,
) -> Result<RfOpen<ChipPhy>, WakeFailure> {
    if let Err(error) = enable_phy_clocks(radio, clocks) {
        return Err(WakeFailure::StillClosed(closed, error));
    }
    let woken = radio_lifecycle::execute_rf_wake_with_hal::<D>(
        timer,
        &mut radio.phy_hardware(),
        phy.state(&closed),
    )
    .await;
    match woken {
        Ok(()) => match radio.disable_phy_modem_clocks(PhyClockModule::Calibration) {
            Ok(()) => Ok(closed.open()),
            Err(error) => Err(WakeFailure::Clock(error, closed.fail())),
        },
        Err(error) => Err(WakeFailure::Failed(error, closed.fail())),
    }
}

/// Select the Wi-Fi channel: `phy_chip_set_chan`, over the channel
/// operation's registers.
pub async fn select_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut RadioChannelHal<'_, P>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<ChipPhy>,
    observer: &mut O,
) -> Result<RfOpen<ChipPhy>, ChannelFailure> {
    match select_phy_channel_with_hal::<D, _, _>(
        timer,
        phy.state_mut(),
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
    {
        Ok(()) => Ok(open),
        Err(error) => Err(ChannelFailure {
            error,
            vacate: open.fail(),
        }),
    }
}

/// Switch the Wi-Fi channel with the MAC's restart around it.
pub async fn switch_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + crate::executor::wait::WaitTally
     ),
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut RadioChannelHal<'_, P>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<ChipPhy>,
    observer: &mut O,
) -> Result<RfOpen<ChipPhy>, ChannelFailure> {
    match switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
        timer,
        phy.state_mut(),
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
    {
        Ok(()) => Ok(open),
        Err(error) => Err(ChannelFailure {
            error,
            vacate: open.fail(),
        }),
    }
}

/// Run one tracking pass with the protocols running, as the vendor's
/// `phy_param_track_tot` does under its PHY lock: the lent domain
/// serializes the radio, and the tracking graph brackets its RF-sensitive
/// regions with the grant-protect request. The pass's instant is the
/// state's last tracking.
pub async fn track<B: PlatformClockBackend, P, D: PhyShortDelay, O: PhyTargetObserver>(
    timer: &(
         impl oer_portable_cooperate::CooperativeTimer
         + oer_portable_settle::SettleCounter
         + oer_portable_clock::Clock
         + crate::executor::wait::WaitTally
     ),
    radio: &mut RadioDomain<B>,
    phy: &mut PhyHandle<'_, ChipPhy>,
    open: RfOpen<ChipPhy>,
    platform: &mut P,
    observer: O,
) -> Result<(RfOpen<ChipPhy>, PhyParamTrackingOutcome), TrackingFailure> {
    let mut tracking = PhyParamTrackingTransition::new(
        crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(phy.state(&open)),
    );
    let result = {
        let mut loan = radio.phy_loan();
        let mut port =
            TargetPhyParamTrackingPort::<_, D, _, _>::new(platform, &mut loan, timer, observer);
        run_phy_param_tracking(&mut tracking, phy.state_mut(), &mut port)
            .await
            .map_err(TargetPhyParamTrackingError::Run)
    };
    match result {
        Ok(outcome) => {
            phy.state_mut()
                .mark_tracked(oer_portable_clock::Clock::now(timer));
            Ok((open, outcome))
        }
        Err(error) => Err(TrackingFailure {
            error,
            vacate: open.fail(),
        }),
    }
}

/// Enable the Wi-Fi baseband receive path: the vendor `phy_wifi_enable_set`
/// with `true`, which ESP-IDF's `enable_phy_with_wifi_rx` runs after the PHY
/// is enabled.
pub fn enable_wifi_rx<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B>,
    _open: &RfOpen<ChipPhy>,
) {
    radio.enable_wifi_rx();
}

/// Disable the Wi-Fi baseband receive path: `phy_wifi_enable_set` with
/// `false`, before Wi-Fi leaves the PHY.
pub fn disable_wifi_rx<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B>,
    _open: &RfOpen<ChipPhy>,
) {
    radio.disable_wifi_rx();
}
