//! The ESP32-S31 backend: the radio domain, its PHY and the Wi-Fi client's
//! shared steps, in the order of the driver's cold start and release.
//!
//! | Request | Steps |
//! | --- | --- |
//! | `prepare` | register the PHY domain, or wake closed RF, or nothing |
//! | `enter` | coexistence, common power, Wi-Fi module clocks, initialized status, PHY client join |
//! | `track` | initial tracking when due, Wi-Fi RX enable, initial channel, TX power profile, MAC cold init |
//! | `exit` | Wi-Fi RX disable, PHY client leave, module clocks and initialized status, coexistence, common power |
//! | `close_rf` | close RF after the last member |
//! | `track_tick` | the periodic tracking when due |
//!
//! Every failure is classified as the client contract's [`Failure`] table
//! states; a `ClientFailed` site says why the shared domain is not touched.

use oer_esp32s31_hal::power::PlatformClockBackend;
use oer_esp32s31_phy::executor::boundary::Boundary;

use oer_esp32s31_hal::ieee80211::init::{MacCoexPti, initialize_wifi_mac};
use oer_esp32s31_hal::{
    domain::{
        CoexAbsent, Coexisting, CommonPower, CommonRadioPowerError, ModemClockError,
        PlatformClockProvider, RadioDomain, WifiClient,
    },
    ieee80211::client::{WifiClocked, WifiCold, WifiPowered},
};
use oer_esp32s31_phy::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyTrackingError, ConcurrentRfError,
    ConcurrentWifiChannelError, PhyCalibrationTrackingFailure, PhyCalibrationTrackingRunError,
    PhyParamTrackingPortError, PhyParamTrackingRunError, PhyRegisterConfig, PhyRegisterFailure,
    PhyRegisterRunError, PhyTargetObserver, PhyTargetPortError, RomShortDelay,
    TargetPhyParamTrackingError, TargetPhyRegisterError, close_concurrent_rf,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, phy_ready},
    maintain_concurrent_phy, register_concurrent_phy, select_concurrent_wifi_channel,
    track_concurrent_phy, wake_concurrent_rf,
    wifi_client::{WifiPhyMembership, join_wifi, leave_wifi, set_wifi_rx},
};
use oer_esp32s31_radio_client::{
    CalibrationTrackingFailure, CalibrationTrackingFault, DomainHealth, Failed, Failure, LpClock,
    PhyFault, PhyPreparation, Poison, PoisonCause, RcSlow, RegistrationFailure, RegistrationFault,
    RequestKind,
    wifi::{
        RetuneReport, TrackReport, Wifi, WifiJoin, WifiJoined, WifiLeave, WifiLeft, WifiRetune,
        WifiRetuned, WifiReturned, WifiTrack, WifiTracked,
    },
};

use crate::owned_operation;

use crate::{
    ClientBackend, CloseFault, Exit, Fault, LastClientLeft, PhyDomain,
    mac_sources::{MAC_COEX_EVENTS, MacSources},
};

mod modem_sleep;
mod retune;

/// Wi-Fi's request `request` poisoned the domain with `cause`.
pub(crate) const fn wifi_poisoned(request: RequestKind, cause: PoisonCause) -> Failure {
    Failure::Poisoned(Poison::request(
        oer_esp32s31_hal::domain::RadioClient::Wifi,
        request,
        cause,
    ))
}

/// The client-contract name of a started PHY transaction's failure.
pub(crate) const fn phy_fault(error: PhyTargetPortError) -> PhyFault {
    match error {
        PhyTargetPortError::HardwareEdgeTimedOut => PhyFault::EdgeTimedOut,
        PhyTargetPortError::HardwareCapabilityUnavailable => PhyFault::CapabilityUnavailable,
        PhyTargetPortError::HardwareInvariant => PhyFault::HardwareInvariant,
        PhyTargetPortError::RfOperationLimit => PhyFault::RfOperationLimit,
        PhyTargetPortError::UnexpectedBinding => PhyFault::UnexpectedBinding,
        PhyTargetPortError::RegistrationEpochMismatch => PhyFault::RegistrationEpochMismatch,
        PhyTargetPortError::DeadlineOverflow => PhyFault::DeadlineOverflow,
    }
}

/// The poison cause of a tracking transaction that did not settle.
pub(crate) const fn tracking_cause(error: ConcurrentPhyTrackingError) -> PoisonCause {
    match error {
        ConcurrentPhyTrackingError::Rejected(error) => PoisonCause::TrackingRefused(error),
        ConcurrentPhyTrackingError::Failed(error) => match error {
            TargetPhyParamTrackingError::EpochMismatch => {
                PoisonCause::Tracking(PhyFault::RegistrationEpochMismatch)
            }
            TargetPhyParamTrackingError::Run(PhyParamTrackingRunError::Port(
                PhyParamTrackingPortError::Step(error),
            )) => PoisonCause::Tracking(phy_fault(error)),
            TargetPhyParamTrackingError::Run(PhyParamTrackingRunError::Port(
                PhyParamTrackingPortError::CalibrationTracking(error),
            )) => PoisonCause::CalibrationTracking(calibration_tracking_fault(error)),
            TargetPhyParamTrackingError::Run(
                PhyParamTrackingRunError::Transition(_) | PhyParamTrackingRunError::ParentEdgeLimit,
            )
            | TargetPhyParamTrackingError::MissingCompletedOwner => {
                PoisonCause::Tracking(PhyFault::Sequence)
            }
        },
    }
}

/// The fault of the calibration-tracking child of PHY tracking.
const fn calibration_tracking_fault(
    error: PhyCalibrationTrackingRunError<PhyTargetPortError>,
) -> CalibrationTrackingFault {
    match error {
        PhyCalibrationTrackingRunError::Port(error) => {
            CalibrationTrackingFault::Step(phy_fault(error))
        }
        PhyCalibrationTrackingRunError::Transition(_)
        | PhyCalibrationTrackingRunError::ParentEdgeLimit => {
            CalibrationTrackingFault::Step(PhyFault::Sequence)
        }
        PhyCalibrationTrackingRunError::Radio(failure) => {
            CalibrationTrackingFault::Radio(match failure {
                PhyCalibrationTrackingFailure::PbusClearTimedOut(_) => {
                    CalibrationTrackingFailure::PbusClearTimedOut
                }
                PhyCalibrationTrackingFailure::Dcode(_) => CalibrationTrackingFailure::Dcode,
                PhyCalibrationTrackingFailure::RxGain(_) => CalibrationTrackingFailure::RxGain,
                PhyCalibrationTrackingFailure::Channel(_) => CalibrationTrackingFailure::Channel,
                PhyCalibrationTrackingFailure::TxDcPwdet(_) => {
                    CalibrationTrackingFailure::TxDcPwdet
                }
            })
        }
    }
}

/// The poison cause of an RF wake or close that did not complete.
const fn rf_cause(error: ConcurrentRfError) -> PoisonCause {
    match error {
        ConcurrentRfError::Rejected(error) => PoisonCause::Domain(error),
        ConcurrentRfError::Recoverable(error) | ConcurrentRfError::Failed(error) => {
            PoisonCause::Rf(phy_fault(error))
        }
        ConcurrentRfError::Clock(error) => PoisonCause::ModemClock(error),
    }
}

/// The poison cause of a registration that did not complete cleanly.
fn registration_cause(failure: &ConcurrentPhyRegisterFailure) -> PoisonCause {
    match failure {
        ConcurrentPhyRegisterFailure::Rejected(error) => PoisonCause::Domain(*error),
        ConcurrentPhyRegisterFailure::Failed { failure, .. } => {
            PoisonCause::Registration(match failure.error() {
                TargetPhyRegisterError::Run(PhyRegisterRunError::Port(error)) => {
                    RegistrationFault::Step(phy_fault(error))
                }
                TargetPhyRegisterError::Run(PhyRegisterRunError::Lowering { stage, error }) => {
                    RegistrationFault::Lowering { stage, error }
                }
                TargetPhyRegisterError::Run(PhyRegisterRunError::Radio(failure)) => {
                    RegistrationFault::Radio(match failure {
                        PhyRegisterFailure::I2cMasterResetTimedOut { index, samples } => {
                            RegistrationFailure::I2cMasterResetTimedOut { index, samples }
                        }
                        PhyRegisterFailure::Rf(_) => RegistrationFailure::Rf,
                        PhyRegisterFailure::Baseband(_) => RegistrationFailure::Baseband,
                        PhyRegisterFailure::Temperature(_) => RegistrationFailure::Temperature,
                        PhyRegisterFailure::FinalI2cDeadlineExceeded => {
                            RegistrationFailure::FinalI2cDeadlineExceeded
                        }
                    })
                }
                TargetPhyRegisterError::Run(PhyRegisterRunError::Transition(_))
                | TargetPhyRegisterError::MissingCompletedModelOwner => {
                    RegistrationFault::Step(PhyFault::Sequence)
                }
            })
        }
        ConcurrentPhyRegisterFailure::CalibrationClock { error, .. } => {
            PoisonCause::ModemClock(*error)
        }
    }
}

/// Where RF stands, as only this backend changes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Rf {
    Unregistered,
    Open,
    Closed,
}

enum Coex {
    Absent(CoexAbsent<WifiClient>),
    Coexisting(Coexisting<WifiClient>),
}

/// The Wi-Fi client's shared state, which the actor holds for it.
struct WifiShare {
    /// `None` only after an invariant broke; the domain is then poisoned.
    coex: Option<Coex>,
    power: Option<CommonPower<WifiClient>>,
    membership: Option<WifiPhyMembership>,
    tracking_due: bool,
}

/// The ESP32-S31 radio domain as the radio system's backend.
///
/// `C` provides the platform clocks, `T` is the PHY's time and `O` observes
/// the PHY's target work.
pub struct Esp32s31Radio<C: PlatformClockProvider, T, O, S> {
    radio: Option<RadioDomain<C::Backend, ConcurrentPhy>>,
    radio_fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
    clocks: C,
    timer: T,
    pilot: S,
    observer: O,
    /// The first registration's configuration, until it is used.
    registration: Option<PhyRegisterConfig>,
    rf: Rf,
    wifi: WifiShare,
    #[cfg(feature = "bluetooth")]
    bluetooth_attached: bool,
}

impl<C, T, O, S> Esp32s31Radio<C, T, O, S>
where
    C: PlatformClockProvider,
    T: oer_cooperate::CooperativeTimer,
    O: PhyTargetObserver + Clone,
    S: oer_esp32s31_phy::sar_pilot::Service,
{
    /// The backend over the split radio domain and Wi-Fi's coexistence
    /// state; `registration` configures the first PHY registration.
    pub fn new(
        radio: RadioDomain<C::Backend, ConcurrentPhy>,
        wifi_coex: CoexAbsent<WifiClient>,
        clocks: C,
        timer: T,
        observer: O,
        registration: PhyRegisterConfig,
        pilot: S,
    ) -> Self {
        let radio_fault = radio.fault_handle();
        Self {
            radio: Some(radio),
            radio_fault,
            clocks,
            timer,
            observer,
            registration: Some(registration),
            pilot,
            rf: Rf::Unregistered,
            #[cfg(feature = "bluetooth")]
            bluetooth_attached: false,
            wifi: WifiShare {
                coex: Some(Coex::Absent(wifi_coex)),
                power: None,
                membership: None,
                tracking_due: false,
            },
        }
    }

    fn micros_since(&self, start: oer_time::Instant) -> u64 {
        self.timer
            .now()
            .saturating_duration_since(start)
            .as_micros()
    }

    fn enter_coex(&mut self) -> bool {
        match self.wifi.coex.take() {
            Some(Coex::Absent(absent)) => {
                let (coexisting, _participants) =
                    require_radio(self.radio.as_mut(), self.radio_fault).enter_coex(absent);
                self.wifi.coex = Some(Coex::Coexisting(coexisting));
                true
            }
            other => {
                self.wifi.coex = other;
                false
            }
        }
    }

    fn leave_coex(&mut self) {
        if let Some(Coex::Coexisting(coexisting)) = self.wifi.coex.take() {
            let (absent, _participants) =
                require_radio(self.radio.as_mut(), self.radio_fault).leave_coex(coexisting);
            self.wifi.coex = Some(Coex::Absent(absent));
        }
    }

    /// Leave coexistence, then common power, as the HAL requires.
    fn power_down(&mut self, powered: WifiPowered) -> Result<WifiCold, WifiReturned> {
        self.leave_coex();
        let (Some(Coex::Absent(absent)), Some(power)) = (&self.wifi.coex, self.wifi.power.take())
        else {
            return Err(WifiReturned::Powered(powered));
        };
        match powered.power_down(
            require_radio(self.radio.as_mut(), self.radio_fault),
            power,
            absent,
        ) {
            Ok(cold) => Ok(cold),
            Err(failure) => {
                let (powered, power) = failure.into_owner();
                self.wifi.power = Some(power);
                Err(WifiReturned::Powered(powered))
            }
        }
    }

    /// Release the module clocks and initialized status, then power down.
    fn roll_back(&mut self, clocked: WifiClocked) -> Result<WifiCold, WifiReturned> {
        match clocked.release_clocks(require_radio(self.radio.as_mut(), self.radio_fault)) {
            Ok(released) => self.power_down(released.into_powered()),
            Err((stopped, obligation)) => Err(WifiReturned::ClockPoisoned {
                stopped,
                obligation,
            }),
        }
    }

    /// Roll a clocked client back to cold: `ClientFailed` when the rollback
    /// completed, `Poisoned` when it did not.
    fn client_failed(&mut self, clocked: WifiClocked) -> Failed<WifiReturned> {
        match self.roll_back(clocked) {
            Ok(cold) => Failed {
                returned: WifiReturned::Cold(cold),
                cause: Failure::ClientFailed,
            },
            Err(returned) => Failed {
                returned,
                cause: wifi_poisoned(RequestKind::Join, PoisonCause::Rollback),
            },
        }
    }
}

/// Wi-Fi's leave poisoned the domain with `cause`.
const fn leave_poisoned(cause: PoisonCause) -> DomainHealth {
    DomainHealth::Poisoned(Poison::request(
        oer_esp32s31_hal::domain::RadioClient::Wifi,
        RequestKind::Leave,
        cause,
    ))
}

impl<C, T, O, S> PhyDomain for Esp32s31Radio<C, T, O, S>
where
    C: PlatformClockProvider,
    T: oer_cooperate::CooperativeTimer,
    O: PhyTargetObserver + Clone,
    S: oer_esp32s31_phy::sar_pilot::Service,
{
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault> {
        let start = self.timer.now();
        match self.rf {
            Rf::Open => Ok(PhyPreparation::AlreadyOpen),
            Rf::Closed => {
                // Any failure, a rejection included, leaves the domain's
                // state unknown to the actor: it poisons.
                let operation = wake_concurrent_rf::<_, RomShortDelay>(
                    &self.timer,
                    require_radio(self.radio.take(), self.radio_fault),
                    &self.clocks,
                );
                owned_operation::run(operation, &mut self.radio, owned_operation::restore)
                    .await
                    .map_err(|error| Fault(rf_cause(error)))?;
                self.rf = Rf::Open;
                Ok(PhyPreparation::Woke {
                    micros: self.micros_since(start),
                })
            }
            Rf::Unregistered => {
                let config = self
                    .registration
                    .take()
                    .ok_or(Fault(PoisonCause::Custody))?;
                let mut platform = ();
                let operation = register_concurrent_phy::<_, (), RomShortDelay, O>(
                    &self.timer,
                    require_radio(self.radio.take(), self.radio_fault),
                    &mut platform,
                    &self.clocks,
                    config,
                    self.observer.clone(),
                );
                let outcome =
                    owned_operation::run(operation, &mut self.radio, finish_registration).await?;
                self.rf = Rf::Open;
                Ok(PhyPreparation::Registered {
                    calibration_path: outcome.calibration_path,
                    micros: self.micros_since(start),
                    #[cfg(oer_trace)]
                    outcome,
                })
            }
        }
    }

    async fn close_rf(&mut self, last: LastClientLeft) -> Result<(), CloseFault> {
        let LastClientLeft(()) = last;
        let mut platform = ();
        let operation = close_concurrent_rf::<_, (), RomShortDelay>(
            &self.timer,
            require_radio(self.radio.take(), self.radio_fault),
            &mut platform,
        );
        let result =
            owned_operation::run(operation, &mut self.radio, owned_operation::restore).await;
        match result {
            Ok(()) => {
                self.rf = Rf::Closed;
                Ok(())
            }
            // Rejected before any access, or every issued transaction
            // completed: RF stays open.
            Err(ConcurrentRfError::Rejected(_) | ConcurrentRfError::Recoverable(_)) => {
                Err(CloseFault::Recoverable)
            }
            Err(error @ (ConcurrentRfError::Failed(_) | ConcurrentRfError::Clock(_))) => {
                Err(CloseFault::Poisoned(rf_cause(error)))
            }
        }
    }

    fn tracking_deadline(&self) -> Option<oer_time::Instant> {
        let micros = require_radio(self.radio.as_ref(), self.radio_fault)
            .attachment()
            .client_snapshot()?
            .next_tracking_deadline_micros()
            .ok()??;
        Some(oer_time::Instant::from_micros(micros))
    }

    async fn track_tick(&mut self) -> Result<(), Fault> {
        // A rejected tick (the tracking clock ran backwards) would fall due
        // again at once; it poisons like a failed one.
        let mut platform = ();
        let operation = track_concurrent_phy::<_, (), RomShortDelay, O>(
            require_radio(self.radio.take(), self.radio_fault),
            &mut platform,
            &self.timer,
            self.observer.clone(),
            &mut self.pilot,
        );
        let result = owned_operation::run(operation, &mut self.radio, owned_operation::restore)
            .await
            .map(|_tick| ())
            .map_err(|error| Fault(tracking_cause(error)));
        #[cfg(feature = "bluetooth")]
        if result.is_ok() && self.bluetooth_attached {
            self.wifi.tracking_due = false;
        }
        result
    }
}

impl<C, T, O, S> ClientBackend<Wifi, RcSlow> for Esp32s31Radio<C, T, O, S>
where
    C: PlatformClockProvider,
    T: oer_cooperate::CooperativeTimer,
    O: PhyTargetObserver + Clone,
    S: oer_esp32s31_phy::sar_pilot::Service,
{
    async fn enter(
        &mut self,
        join: WifiJoin,
        phy: PhyPreparation,
        _lp: &LpClock<RcSlow>,
    ) -> Result<WifiJoined, Failed<WifiReturned>> {
        #[cfg(feature = "bluetooth")]
        if self.bluetooth_attached {
            if let Err(Fault(cause)) = Boundary::new(self.track_tick()).await {
                return Err(Failed {
                    returned: <Wifi as oer_esp32s31_radio_client::Client>::join_returned(join),
                    cause: wifi_poisoned(RequestKind::Join, cause),
                });
            }
        }
        let WifiJoin { wifi: cold } = join;
        // Wi-Fi enters coexistence before common power.
        if !self.enter_coex() {
            return Err(Failed {
                returned: WifiReturned::Cold(cold),
                cause: wifi_poisoned(RequestKind::Join, PoisonCause::Custody),
            });
        }
        let (powered, power) = match cold.power_up(
            require_radio(self.radio.as_mut(), self.radio_fault),
            &self.clocks,
        ) {
            Ok(powered) => powered,
            Err(failure) => {
                let cause = match failure.error() {
                    // ClientFailed: refused before the power sequence; only
                    // Wi-Fi's coexistence participation changed, and it is
                    // left below.
                    CommonRadioPowerError::AlreadyEntered => Failure::ClientFailed,
                    // The first client's power sequence stopped at a
                    // read-back checkpoint: common power is half applied.
                    // Entry gives no reference back, so a refused release
                    // is not reached here; it would leave power as uncertain.
                    error @ (CommonRadioPowerError::Power(_)
                    | CommonRadioPowerError::Restore(_)
                    | CommonRadioPowerError::Release(_)) => {
                        wifi_poisoned(RequestKind::Join, PoisonCause::CommonPower(error))
                    }
                };
                self.leave_coex();
                return Err(Failed {
                    returned: WifiReturned::Cold(failure.into_owner()),
                    cause,
                });
            }
        };
        self.wifi.power = Some(power);
        let mut clocked = match powered.enable_clocks(
            require_radio(self.radio.as_mut(), self.radio_fault),
            &self.clocks,
        ) {
            Ok(clocked) => clocked,
            Err(failure) => {
                let error = failure.error();
                let powered = failure.into_owner();
                if error == ModemClockError::Poisoned {
                    return Err(Failed {
                        returned: WifiReturned::Powered(powered),
                        cause: wifi_poisoned(RequestKind::Join, PoisonCause::ModemClock(error)),
                    });
                }
                // ClientFailed: the planner refused before any clock write;
                // leaving common power as the client that entered it restores
                // the shared state, and a failed exit poisons.
                return Err(match self.power_down(powered) {
                    Ok(cold) => Failed {
                        returned: WifiReturned::Cold(cold),
                        cause: Failure::ClientFailed,
                    },
                    Err(returned) => Failed {
                        returned,
                        cause: wifi_poisoned(RequestKind::Join, PoisonCause::Rollback),
                    },
                });
            }
        };
        if let Err(error) =
            clocked.set_initialized(require_radio(self.radio.as_mut(), self.radio_fault), true)
        {
            // The modem clocks are poisoned.
            return Err(Failed {
                returned: WifiReturned::Clocked(clocked),
                cause: wifi_poisoned(RequestKind::Join, PoisonCause::ModemClock(error)),
            });
        }
        let joined = match phy_ready(require_radio(self.radio.as_mut(), self.radio_fault)) {
            Ok(ready) => join_wifi(vacant_wifi(ready), &clocked, &self.timer)
                .map(|joined| (joined.membership, joined.acquired))
                .map_err(|refused| refused.error),
            Err(error) => Err(error),
        };
        match joined {
            Ok((membership, acquired)) => {
                self.wifi.membership = Some(membership);
                self.wifi.tracking_due = acquired == ConcurrentAcquire::TrackingDue;
                Ok(WifiJoined { wifi: clocked, phy })
            }
            // ClientFailed: the PHY refused the client before any access;
            // only Wi-Fi's clocks, power and coexistence changed, and the
            // rollback restores them.
            Err(_) => Err(self.client_failed(clocked)),
        }
    }

    async fn track(&mut self, track: WifiTrack) -> Result<WifiTracked, Failed<WifiClocked>> {
        #[cfg(feature = "bluetooth")]
        if self.bluetooth_attached {
            if let Err(Fault(cause)) = Boundary::new(self.track_tick()).await {
                return Err(Failed {
                    returned: track.wifi,
                    cause: wifi_poisoned(RequestKind::Track, cause),
                });
            }
        }
        let WifiTrack {
            mut wifi,
            channel,
            tx_ceiling,
            mac,
            slow_clock,
            delay_random,
        } = track;
        let Some(membership) = self.wifi.membership.take() else {
            return Err(Failed {
                returned: wifi,
                cause: wifi_poisoned(RequestKind::Track, PoisonCause::Custody),
            });
        };
        let tracking_micros = if self.wifi.tracking_due {
            let start = self.timer.now();
            let mut platform = ();
            let operation = maintain_concurrent_phy::<_, (), RomShortDelay, O>(
                &self.timer,
                require_radio(self.radio.take(), self.radio_fault),
                &mut platform,
                self.observer.clone(),
                &mut self.pilot,
            );
            let result =
                owned_operation::run(operation, &mut self.radio, owned_operation::restore).await;
            match result {
                Ok(_outcome) => {
                    self.wifi.tracking_due = false;
                    Some(self.micros_since(start))
                }
                Err(error) => {
                    self.wifi.membership = Some(membership);
                    return Err(Failed {
                        returned: wifi,
                        cause: match error {
                            ConcurrentPhyTrackingError::Rejected(_) => Failure::Refused,
                            ConcurrentPhyTrackingError::Failed(_) => {
                                wifi_poisoned(RequestKind::Track, tracking_cause(error))
                            }
                        },
                    });
                }
            }
        } else {
            None
        };

        set_wifi_rx(
            require_radio(self.radio.as_mut(), self.radio_fault),
            &membership,
            true,
        );
        let lowered = crate::channel::lower_wifi_channel(channel.channel());
        let mut observer = self.observer.clone();
        let mut platform = ();
        let transaction = require_radio(self.radio.take(), self.radio_fault)
            .detach(oer_esp32s31_hal::domain::transaction::Operation::Channel);
        let channel = wifi.channel_transaction(&mut platform, transaction);
        let operation = select_concurrent_wifi_channel::<RomShortDelay, _, _, _>(
            &self.timer,
            lowered.channel_or_frequency,
            lowered.cbw,
            channel,
            &mut observer,
        );
        let selected =
            owned_operation::run(operation, &mut self.radio, owned_operation::restore).await;
        if let Err(error) = selected {
            let cause = match error {
                ConcurrentWifiChannelError::Rejected(_) => {
                    // ClientFailed: the channel transaction did not start; the
                    // only shared write is Wi-Fi's own RX enable, cleared again.
                    set_wifi_rx(
                        require_radio(self.radio.as_mut(), self.radio_fault),
                        &membership,
                        false,
                    );
                    Failure::ClientFailed
                }
                ConcurrentWifiChannelError::Failed(error) => {
                    wifi_poisoned(RequestKind::Track, PoisonCause::Channel(phy_fault(error)))
                }
            };
            self.wifi.membership = Some(membership);
            return Err(Failed {
                returned: wifi,
                cause,
            });
        }

        let (tx_power, temperature) = match require_radio(self.radio.as_ref(), self.radio_fault)
            .attachment()
            .phy_state()
        {
            Ok(state) => (
                state
                    .tx_target_power_profile()
                    .with_maximum_quarter_dbm(tx_ceiling.quarter_dbm()),
                state.temperature_observation(),
            ),
            Err(error) => {
                // The channel switch settled the domain; an unsettled one
                // breaks that invariant.
                self.wifi.membership = Some(membership);
                return Err(Failed {
                    returned: wifi,
                    cause: wifi_poisoned(RequestKind::Track, PoisonCause::Domain(error)),
                });
            }
        };

        let mut sources = MacSources {
            pti: MAC_COEX_EVENTS.map(|event| {
                MacCoexPti::new(
                    require_radio(self.radio.as_ref(), self.radio_fault)
                        .coex_pti(event.coex_event())
                        .value(),
                )
                .expect("the coexistence table holds four-bit priorities")
            }),
            tx_power,
            slow_clock,
            delay_random,
        };
        let mac_outcome = {
            let mut hal = wifi.cold_mac_hal(require_radio(self.radio.as_mut(), self.radio_fault));
            // Track prepares MAC configuration and IRQs with its common gates
            // held. Only the later runtime owner can supply an RX-ring proof.
            initialize_wifi_mac(&mut sources, &mut hal, &self.timer, mac).await
        };
        match mac_outcome {
            Ok(mac) => {
                self.wifi.membership = Some(membership);
                Ok(WifiTracked {
                    wifi,
                    report: TrackReport {
                        probe_priority: crate::mac_sources::probe_priority(
                            require_radio(self.radio.as_ref(), self.radio_fault).coex_pti_table(),
                        ),
                        tracking_micros,
                        temperature,
                        tx_power,
                        mac,
                    },
                })
            }
            Err(_handshake) => {
                // ClientFailed: the handshake times out after only the
                // WIFIMAC reset pulse in the shared MODEM_RST_CONF, which
                // sets and clears Wi-Fi's own reset bit and leaves the
                // register as it was; the half-started MAC is Wi-Fi's
                // partition. The TX power queries come later and read only
                // the owned profile. Wi-Fi's RX enable is cleared again.
                set_wifi_rx(
                    require_radio(self.radio.as_mut(), self.radio_fault),
                    &membership,
                    false,
                );
                self.wifi.membership = Some(membership);
                Err(Failed {
                    returned: wifi,
                    cause: Failure::ClientFailed,
                })
            }
        }
    }

    async fn retune(&mut self, retune: WifiRetune) -> Result<WifiRetuned, Failed<WifiClocked>> {
        #[cfg(feature = "bluetooth")]
        if self.bluetooth_attached {
            if let Err(Fault(cause)) = Boundary::new(self.track_tick()).await {
                return Err(Failed {
                    returned: retune.wifi,
                    cause: wifi_poisoned(RequestKind::Retune, cause),
                });
            }
        }
        let WifiRetune {
            mut wifi,
            channel,
            tx_ceiling,
        } = retune;
        if self.wifi.membership.is_none() {
            return Err(Failed {
                returned: wifi,
                cause: wifi_poisoned(RequestKind::Retune, PoisonCause::Custody),
            });
        }
        let start = self.timer.now();
        let mut observer = self.observer.clone();
        let (radio, result) = retune::select_wifi_channel(
            &self.timer,
            require_radio(self.radio.take(), self.radio_fault),
            &mut wifi,
            channel,
            &mut observer,
        )
        .await;
        self.radio = Some(radio);
        if let Err(error) = result {
            return Err(Failed {
                returned: wifi,
                cause: match error {
                    ConcurrentWifiChannelError::Rejected(_) => Failure::Refused,
                    ConcurrentWifiChannelError::Failed(error) => {
                        wifi_poisoned(RequestKind::Retune, PoisonCause::Channel(phy_fault(error)))
                    }
                },
            });
        }
        let state = match require_radio(self.radio.as_ref(), self.radio_fault)
            .attachment()
            .phy_state()
        {
            Ok(state) => state,
            Err(error) => {
                return Err(Failed {
                    returned: wifi,
                    cause: wifi_poisoned(RequestKind::Retune, PoisonCause::Domain(error)),
                });
            }
        };
        Ok(WifiRetuned {
            wifi,
            report: RetuneReport {
                probe_priority: crate::mac_sources::probe_priority(
                    require_radio(self.radio.as_ref(), self.radio_fault).coex_pti_table(),
                ),
                micros: self.micros_since(start),
                tx_power: state
                    .tx_target_power_profile()
                    .with_maximum_quarter_dbm(tx_ceiling.quarter_dbm()),
                temperature: state.temperature_observation(),
            },
        })
    }

    async fn exit(&mut self, leave: WifiLeave) -> (WifiLeft, Exit) {
        #[cfg(feature = "bluetooth")]
        if self.bluetooth_attached {
            if let Err(Fault(cause)) = Boundary::new(self.track_tick()).await {
                return (
                    oer_esp32s31_radio_client::wifi::WifiLeft {
                        wifi: oer_esp32s31_radio_client::wifi::WifiReturned::Clocked(leave.wifi),
                    },
                    Exit {
                        last: false,
                        health: leave_poisoned(cause),
                    },
                );
            }
        }
        let WifiLeave { wifi } = leave;
        let poisoned = |wifi, last, cause| {
            (
                WifiLeft { wifi },
                Exit {
                    last,
                    health: leave_poisoned(cause),
                },
            )
        };
        let Some(membership) = self.wifi.membership.take() else {
            return poisoned(WifiReturned::Clocked(wifi), false, PoisonCause::Custody);
        };
        set_wifi_rx(
            require_radio(self.radio.as_mut(), self.radio_fault),
            &membership,
            false,
        );
        let last = match leave_wifi(
            require_radio(self.radio.as_mut(), self.radio_fault),
            &wifi,
            membership,
        ) {
            Ok(last) => last,
            Err(failure) => {
                // The member is retired but the PHY still counts Wi-Fi.
                let cause = PoisonCause::Domain(failure.error());
                self.wifi.membership = Some(failure.into_membership());
                return poisoned(WifiReturned::Clocked(wifi), false, cause);
            }
        };
        match self.roll_back(wifi) {
            Ok(cold) => (
                WifiLeft {
                    wifi: WifiReturned::Cold(cold),
                },
                Exit {
                    last,
                    health: DomainHealth::Healthy,
                },
            ),
            Err(returned) => poisoned(returned, last, PoisonCause::Rollback),
        }
    }
}

#[cfg(target_arch = "riscv32")]
fn vacant_wifi<B: PlatformClockBackend>(
    ready: oer_esp32s31_phy::concurrent::PhyReady<'_, B>,
) -> oer_esp32s31_phy::concurrent::PhyClientReady<'_, B, WifiPhyMembership> {
    match ready.client::<WifiPhyMembership>() {
        oer_esp32s31_phy::concurrent::PhyClientAccess::Available(available) => available,
        _ => panic!("Wi-Fi join phase owns the unacquired PHY client"),
    }
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::BluetoothAttachment;

// Project the registration diagnostic before returning to the actor poll.
#[inline(never)]
fn finish_registration<B: oer_esp32s31_hal::power::PlatformClockBackend>(
    destination: &mut Option<RadioDomain<B, ConcurrentPhy>>,
    returned: oer_memory::Placed<
        '_,
        (
            RadioDomain<B, ConcurrentPhy>,
            Result<oer_esp32s31_phy::ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>,
        ),
    >,
) -> Result<oer_esp32s31_phy::PhyRegisterOutcome, Fault> {
    let registration = owned_operation::restore(destination, returned)
        .map_err(|failure| Fault(registration_cause(&failure)))?;
    Ok(registration.outcome())
}

// An absent domain means an owned operation has not returned. Its original
// diagnostic capability retains the source owner even after cancellation.
fn require_radio<R>(
    owner: Option<R>,
    fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
) -> R {
    fault.require_owner(owner)
}
