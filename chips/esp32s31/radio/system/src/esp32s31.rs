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

use oer_esp32s31_hal::{
    domain::{
        CoexAbsent, Coexisting, CommonPower, CommonRadioPowerError, ModemClockError,
        PlatformClockProvider, RadioDomain, WifiClient,
    },
    ieee80211::client::{WifiClocked, WifiCold, WifiPowered},
};
use oer_esp32s31_ieee80211_mac::init::{MacCoexPti, initialize_wifi_mac};
use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentRfError, ConcurrentWifiChannelError, PhyRegisterConfig,
    PhyTargetObserver, RomShortDelay, close_concurrent_rf,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, phy_ready},
    maintain_concurrent_phy, register_concurrent_phy, select_concurrent_wifi_channel,
    track_concurrent_phy, wake_concurrent_rf,
    wifi_client::{WifiPhyMembership, join_wifi, leave_wifi, set_wifi_rx},
};
use oer_esp32s31_radio_client::{
    DomainHealth, Failed, Failure, LpClock, PhyPreparation, RcSlow,
    wifi::{
        TrackReport, Wifi, WifiJoin, WifiJoined, WifiLeave, WifiLeft, WifiReturned, WifiTrack,
        WifiTracked,
    },
};

use crate::{
    ClientBackend, CloseFault, Exit, Fault, LastClientLeft, PhyDomain,
    mac_sources::{MAC_COEX_EVENTS, MacSources},
};

mod modem_sleep;
mod retune;

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
pub struct Esp32s31Radio<C, T, O> {
    radio: RadioDomain<ConcurrentPhy>,
    clocks: C,
    timer: T,
    observer: O,
    /// The first registration's configuration, until it is used.
    registration: Option<PhyRegisterConfig>,
    rf: Rf,
    wifi: WifiShare,
}

impl<C, T, O> Esp32s31Radio<C, T, O>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
{
    /// The backend over the split radio domain and Wi-Fi's coexistence
    /// state; `registration` configures the first PHY registration.
    pub fn new(
        radio: RadioDomain<ConcurrentPhy>,
        wifi_coex: CoexAbsent<WifiClient>,
        clocks: C,
        timer: T,
        observer: O,
        registration: PhyRegisterConfig,
    ) -> Self {
        Self {
            radio,
            clocks,
            timer,
            observer,
            registration: Some(registration),
            rf: Rf::Unregistered,
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
                let (coexisting, _participants) = self.radio.enter_coex(absent);
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
            let (absent, _participants) = self.radio.leave_coex(coexisting);
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
        match powered.power_down(&mut self.radio, power, absent) {
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
        match clocked.release_clocks(&mut self.radio) {
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
                cause: Failure::Poisoned,
            },
        }
    }
}

impl<C, T, O> PhyDomain for Esp32s31Radio<C, T, O>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
{
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault> {
        let start = self.timer.now();
        match self.rf {
            Rf::Open => Ok(PhyPreparation::AlreadyOpen),
            Rf::Closed => {
                // Any failure, a rejection included, leaves the domain's
                // state unknown to the actor: it poisons.
                wake_concurrent_rf::<RomShortDelay>(&self.timer, &mut self.radio, &self.clocks)
                    .await
                    .map_err(|_| Fault)?;
                self.rf = Rf::Open;
                Ok(PhyPreparation::Woke {
                    micros: self.micros_since(start),
                })
            }
            Rf::Unregistered => {
                let config = self.registration.take().ok_or(Fault)?;
                let registration = register_concurrent_phy::<(), RomShortDelay, O>(
                    &self.timer,
                    &mut self.radio,
                    &mut (),
                    &self.clocks,
                    config,
                    self.observer.clone(),
                )
                .await
                .map_err(|_| Fault)?;
                self.rf = Rf::Open;
                let outcome = registration.outcome();
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
        match close_concurrent_rf::<(), RomShortDelay>(&self.timer, &mut self.radio, &mut ()).await
        {
            Ok(()) => {
                self.rf = Rf::Closed;
                Ok(())
            }
            // Rejected before any access, or every issued transaction
            // completed: RF stays open.
            Err(ConcurrentRfError::Rejected(_) | ConcurrentRfError::Recoverable(_)) => {
                Err(CloseFault::Recoverable)
            }
            Err(ConcurrentRfError::Failed(_) | ConcurrentRfError::Clock(_)) => {
                Err(CloseFault::Poisoned)
            }
        }
    }

    fn tracking_deadline(&self) -> Option<oer_time::Instant> {
        let micros = self
            .radio
            .attachment()
            .client_snapshot()?
            .next_tracking_deadline_micros()
            .ok()??;
        Some(oer_time::Instant::from_micros(micros))
    }

    async fn track_tick(&mut self) -> Result<(), Fault> {
        // A rejected tick (the tracking clock ran backwards) would fall due
        // again at once; it poisons like a failed one.
        track_concurrent_phy::<(), RomShortDelay, O>(
            &mut self.radio,
            &mut (),
            &self.timer,
            self.observer.clone(),
        )
        .await
        .map(|_tick| ())
        .map_err(|_| Fault)
    }
}

impl<C, T, O> ClientBackend<Wifi, RcSlow> for Esp32s31Radio<C, T, O>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
{
    async fn enter(
        &mut self,
        join: WifiJoin,
        phy: PhyPreparation,
        _lp: &LpClock<RcSlow>,
    ) -> Result<WifiJoined, Failed<WifiReturned>> {
        let WifiJoin { wifi: cold } = join;
        // Wi-Fi enters coexistence before common power.
        if !self.enter_coex() {
            return Err(Failed {
                returned: WifiReturned::Cold(cold),
                cause: Failure::Poisoned,
            });
        }
        let (powered, power) = match cold.power_up(&mut self.radio, &self.clocks) {
            Ok(powered) => powered,
            Err(failure) => {
                let cause = match failure.error() {
                    // ClientFailed: refused before the power sequence; only
                    // Wi-Fi's coexistence participation changed, and it is
                    // left below.
                    CommonRadioPowerError::AlreadyEntered => Failure::ClientFailed,
                    // The first client's power sequence stopped at a
                    // read-back checkpoint: common power is half applied.
                    CommonRadioPowerError::Power(_) | CommonRadioPowerError::Restore(_) => {
                        Failure::Poisoned
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
        let mut clocked = match powered.enable_clocks(&mut self.radio, &self.clocks) {
            Ok(clocked) => clocked,
            Err(failure) => {
                let error = failure.error();
                let powered = failure.into_owner();
                if error == ModemClockError::Poisoned {
                    return Err(Failed {
                        returned: WifiReturned::Powered(powered),
                        cause: Failure::Poisoned,
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
                        cause: Failure::Poisoned,
                    },
                });
            }
        };
        if clocked.set_initialized(&mut self.radio, true).is_err() {
            // The modem clocks are poisoned.
            return Err(Failed {
                returned: WifiReturned::Clocked(clocked),
                cause: Failure::Poisoned,
            });
        }
        let joined = match phy_ready(&mut self.radio) {
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
                cause: Failure::Poisoned,
            });
        };
        let tracking_micros = if self.wifi.tracking_due {
            let start = self.timer.now();
            match maintain_concurrent_phy::<(), RomShortDelay, O>(
                &self.timer,
                &mut self.radio,
                &mut (),
                self.observer.clone(),
            )
            .await
            {
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
                            ConcurrentPhyTrackingError::Failed(_) => Failure::Poisoned,
                        },
                    });
                }
            }
        } else {
            None
        };

        set_wifi_rx(&mut self.radio, &membership, true);
        let lowered = crate::channel::lower_wifi_channel(channel.channel());
        let mut observer = self.observer.clone();
        let mut platform = ();
        let selected = {
            let (mut channel, phy) =
                wifi.channel_hal_with_attachment(&mut platform, &mut self.radio);
            select_concurrent_wifi_channel::<RomShortDelay, _, _>(
                &self.timer,
                phy,
                lowered.channel_or_frequency,
                lowered.cbw,
                &mut channel,
                &mut observer,
            )
            .await
        };
        if let Err(error) = selected {
            let cause = if matches!(error, ConcurrentWifiChannelError::Rejected(_)) {
                // ClientFailed: the channel transaction did not start; the
                // only shared write is Wi-Fi's own RX enable, cleared again.
                set_wifi_rx(&mut self.radio, &membership, false);
                Failure::ClientFailed
            } else {
                Failure::Poisoned
            };
            self.wifi.membership = Some(membership);
            return Err(Failed {
                returned: wifi,
                cause,
            });
        }

        let (tx_power, temperature) = match self.radio.attachment().phy_state() {
            Ok(state) => (
                state
                    .tx_target_power_profile()
                    .with_maximum_quarter_dbm(tx_ceiling.quarter_dbm()),
                state.temperature_observation(),
            ),
            Err(_) => {
                // The channel switch settled the domain; an unsettled one
                // breaks that invariant.
                self.wifi.membership = Some(membership);
                return Err(Failed {
                    returned: wifi,
                    cause: Failure::Poisoned,
                });
            }
        };

        let mut sources = MacSources {
            pti: MAC_COEX_EVENTS.map(|event| {
                MacCoexPti::new(self.radio.coex_pti(event.coex_event()).value())
                    .expect("the coexistence table holds four-bit priorities")
            }),
            tx_power,
            slow_clock,
            delay_random,
        };
        let mac_outcome = {
            let mut hal = wifi.cold_mac_hal(&mut self.radio);
            initialize_wifi_mac(&mut sources, &mut hal, &self.timer, mac).await
        };
        match mac_outcome {
            Ok(mac) => {
                self.wifi.membership = Some(membership);
                Ok(WifiTracked {
                    wifi,
                    report: TrackReport {
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
                set_wifi_rx(&mut self.radio, &membership, false);
                self.wifi.membership = Some(membership);
                Err(Failed {
                    returned: wifi,
                    cause: Failure::ClientFailed,
                })
            }
        }
    }

    async fn exit(&mut self, leave: WifiLeave) -> (WifiLeft, Exit) {
        let WifiLeave { wifi } = leave;
        let poisoned = |wifi, last| {
            (
                WifiLeft { wifi },
                Exit {
                    last,
                    health: DomainHealth::Poisoned,
                },
            )
        };
        let Some(membership) = self.wifi.membership.take() else {
            return poisoned(WifiReturned::Clocked(wifi), false);
        };
        set_wifi_rx(&mut self.radio, &membership, false);
        let last = match leave_wifi(&mut self.radio, &wifi, membership) {
            Ok(last) => last,
            Err(failure) => {
                // The member is retired but the PHY still counts Wi-Fi.
                self.wifi.membership = Some(failure.into_membership());
                return poisoned(WifiReturned::Clocked(wifi), false);
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
            Err(returned) => poisoned(returned, last),
        }
    }
}

#[cfg(target_arch = "riscv32")]
fn vacant_wifi(
    ready: oer_esp32s31_phy::concurrent::PhyReady<'_>,
) -> oer_esp32s31_phy::concurrent::PhyClientReady<'_, WifiPhyMembership> {
    match ready.client::<WifiPhyMembership>() {
        oer_esp32s31_phy::concurrent::PhyClientAccess::Available(available) => available,
        _ => panic!("Wi-Fi join phase owns the unacquired PHY client"),
    }
}
