//! Reusable role-neutral Wi-Fi bring-up as a client of the radio arbiter.
//!
//! This boundary owns the production ordering shared by all firmware, in two
//! halves. The radio system that owns the arbiter first
//! prepares the shared PHY (registration, or wake of closed RF) and enables
//! coexistence. [`power_up_esp32s31_wifi`] then enters common radio power and
//! enables the Wi-Fi module clocks and initialized status, and
//! [`start_esp32s31_wifi`] proves the domain ready and enters the Wi-Fi PHY
//! client with its initial tracking, the Wi-Fi RX enable and the initial
//! channel. After a full common-power cycle, Wi-Fi works again only when the
//! RF wake and coexistence precede the repeated power entry. Board
//! token construction and diagnostics remain caller policy.

use crate::channel::lower_wifi_channel;

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiCold, WifiPowered},
    root::WifiPartition,
    shared_radio::{
        CommonPower, CommonRadioPowerError, ModemClockError, PlatformClockProvider,
        SharedRadioLease, WifiClient,
    },
};

use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentWifiChannelError, PhyShortDelay, PhyTargetObserver,
    PhyTxTargetPowerProfile,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, phy_ready},
    maintain_concurrent_phy, select_concurrent_wifi_channel,
    tracking::PhyParamTrackingOutcome,
    wifi_client::{WifiPhyMembership, join_wifi, set_wifi_rx},
};

use oer_ieee80211_mac::channel::WifiChannel;

/// Application-selected inputs for one Wi-Fi bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiColdStartConfig {
    pub initial_channel: WifiChannel,
    pub maximum_tx_power_quarter_dbm: i8,
}

impl WifiColdStartConfig {
    pub const fn new(initial_channel: WifiChannel) -> Self {
        Self {
            initial_channel,
            maximum_tx_power_quarter_dbm: i8::MAX,
        }
    }

    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = maximum;
        self
    }
}

/// Observable result of the finite bring-up without telemetry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiColdStartReport {
    pub initial_channel: WifiChannel,
    /// None when joining the domain did not require tracking.
    pub initial_tracking: Option<PhyParamTrackingOutcome>,
    /// The calibrated TX target-power profile under the configured ceiling;
    /// every Wi-Fi TX vector takes its power from it.
    pub tx_power: PhyTxTargetPowerProfile,
}

/// Complete owner set returned at the cold-MAC boundary.
pub struct WifiColdStart<W> {
    pub(crate) clocked: WifiClocked,
    pub(crate) membership: WifiPhyMembership,
    pub(crate) platform: W,
    pub(crate) report: WifiColdStartReport,
}

impl<W> WifiColdStart<W> {
    pub const fn report(&self) -> WifiColdStartReport {
        self.report
    }
}

/// Why the bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiColdStartError {
    /// Common radio power was rejected or failed a read-back checkpoint.
    Power(CommonRadioPowerError),
    /// The Wi-Fi module clocks or the initialized status could not change.
    Clocks(ModemClockError),
    /// The shared PHY domain rejected the Wi-Fi client.
    Join(ConcurrentPhyError),
    /// The initial tracking was rejected or failed.
    InitialTracking(ConcurrentPhyTrackingError),
    /// The initial channel was rejected or failed.
    InitialChannel(ConcurrentWifiChannelError),
}

/// Wi-Fi inside common power with its module clocks on and its
/// initialized status set: the half of the bring-up before the shared PHY
/// is prepared for it.
#[must_use = "the initialized Wi-Fi client must start or leave common power"]
pub struct WifiInitialized<W> {
    clocked: WifiClocked,
    platform: W,
}

impl<W> WifiInitialized<W> {
    /// The clocked client and Wi-Fi's MAC platform.
    pub fn into_parts(self) -> (WifiClocked, W) {
        (self.clocked, self.platform)
    }
}

/// The Wi-Fi owners a failed power-up returns, at the stage it reached,
/// with the power proof once power was entered.
#[must_use = "a failed Wi-Fi power-up still owns the Wi-Fi partition"]
pub enum WifiPowerUpFrontier<W> {
    /// Nothing changed; the partition is unpowered.
    Cold { cold: WifiCold, platform: W },
    /// Common power is held; the Wi-Fi clocks are off.
    Powered {
        powered: WifiPowered,
        power: CommonPower<WifiClient>,
        platform: W,
    },
    /// The Wi-Fi clocks are on; the initialized status did not change.
    Clocked {
        clocked: WifiClocked,
        power: CommonPower<WifiClient>,
        platform: W,
    },
}

/// Failed power-up retaining the Wi-Fi owners at their exact stage.
#[must_use = "a failed Wi-Fi power-up still owns the Wi-Fi partition"]
pub struct WifiPowerUpFailure<W> {
    error: WifiColdStartError,
    frontier: WifiPowerUpFrontier<W>,
}

impl<W> WifiPowerUpFailure<W> {
    pub const fn error(&self) -> WifiColdStartError {
        self.error
    }

    pub fn into_frontier(self) -> WifiPowerUpFrontier<W> {
        self.frontier
    }
}

/// The Wi-Fi owners a failed start returns, at the stage it reached. The
/// power proof stays with the radio system, which holds it in coexistence.
#[must_use = "a failed Wi-Fi bring-up still owns the Wi-Fi partition"]
pub enum WifiColdStartFrontier<W> {
    /// The Wi-Fi clocks are on; Wi-Fi is not a PHY client.
    Clocked { clocked: WifiClocked, platform: W },
    /// Wi-Fi is a PHY client with its baseband receive path disabled, unless
    /// a started PHY transaction failed.
    Joined {
        clocked: WifiClocked,
        membership: WifiPhyMembership,
        platform: W,
    },
}

/// Failed bring-up retaining the Wi-Fi owners at their exact stage.
#[must_use = "a failed Wi-Fi bring-up still owns the Wi-Fi partition"]
pub struct WifiColdStartFailure<W> {
    error: WifiColdStartError,
    frontier: WifiColdStartFrontier<W>,
}

impl<W> WifiColdStartFailure<W> {
    pub const fn error(&self) -> WifiColdStartError {
        self.error
    }

    /// Whether a started PHY transaction failed: the shared domain is
    /// poisoned and the radio requires reset.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        matches!(
            self.error,
            WifiColdStartError::InitialTracking(ConcurrentPhyTrackingError::Failed(_))
                | WifiColdStartError::InitialChannel(ConcurrentWifiChannelError::Failed(_))
        )
    }

    pub fn into_frontier(self) -> WifiColdStartFrontier<W> {
        self.frontier
    }
}

/// Enter common radio power, enable the Wi-Fi module clocks and set the
/// initialized status: the first half of the bring-up. The returned proof
/// is what the client's release consumes.
///
/// # Errors
///
/// A step failed; the failure returns the owners at the stage reached.
#[allow(
    clippy::result_large_err,
    reason = "failure must return the complete Wi-Fi frontier"
)]
pub fn power_up_esp32s31_wifi<W>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider,
    partition: WifiPartition,
    wifi_platform: W,
) -> Result<(WifiInitialized<W>, CommonPower<WifiClient>), WifiPowerUpFailure<W>> {
    let (powered, power) = match WifiCold::from_partition(partition).power_up(lease, clocks) {
        Ok(powered) => powered,
        Err(failure) => {
            return Err(WifiPowerUpFailure {
                error: WifiColdStartError::Power(failure.error()),
                frontier: WifiPowerUpFrontier::Cold {
                    cold: failure.into_owner(),
                    platform: wifi_platform,
                },
            });
        }
    };
    let mut clocked = match powered.enable_clocks(lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            return Err(WifiPowerUpFailure {
                error: WifiColdStartError::Clocks(failure.error()),
                frontier: WifiPowerUpFrontier::Powered {
                    powered: failure.into_owner(),
                    power,
                    platform: wifi_platform,
                },
            });
        }
    };
    if let Err(error) = clocked.set_initialized(lease, true) {
        return Err(WifiPowerUpFailure {
            error: WifiColdStartError::Clocks(error),
            frontier: WifiPowerUpFrontier::Clocked {
                clocked,
                power,
                platform: wifi_platform,
            },
        });
    }
    Ok((
        WifiInitialized {
            clocked,
            platform: wifi_platform,
        },
        power,
    ))
}

/// Bring the initialized Wi-Fi client up on the shared PHY domain the shared
/// radio's owner prepared: the second half of the bring-up.
///
/// `platform` is the PHY platform token of the shared radio's owner. Initial
/// tracking runs at once, as the domain's periodic tracking does.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result: cancelling during
/// tracking or the channel transaction leaves the shared PHY ambiguous.
#[must_use = "Wi-Fi bring-up must be driven to a terminal result"]
pub async fn start_esp32s31_wifi<P, W, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    initialized: WifiInitialized<W>,
    config: WifiColdStartConfig,
    mut observer: O,
    timer: &impl oer_time::Timer,
) -> Result<WifiColdStart<W>, WifiColdStartFailure<W>>
where
    D: PhyShortDelay,
    O: PhyTargetObserver + Clone,
{
    let WifiInitialized {
        mut clocked,
        platform: wifi_platform,
    } = initialized;
    // The shared radio's owner prepared the domain; prove it ready under the
    // lease the join uses.
    let joined = match phy_ready(lease) {
        Ok(ready) => join_wifi(ready, &clocked, timer)
            .map(|joined| (joined.membership, joined.acquired))
            .map_err(|refused| refused.error),
        Err(error) => Err(error),
    };
    let (membership, acquired) = match joined {
        Ok(joined) => joined,
        Err(error) => {
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::Join(error),
                frontier: WifiColdStartFrontier::Clocked {
                    clocked,
                    platform: wifi_platform,
                },
            });
        }
    };
    let initial_tracking = if acquired == ConcurrentAcquire::TrackingDue {
        match maintain_concurrent_phy::<P, D, _>(timer, lease, platform, observer.clone()).await {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                return Err(WifiColdStartFailure {
                    error: WifiColdStartError::InitialTracking(error),
                    frontier: WifiColdStartFrontier::Joined {
                        clocked,
                        membership,
                        platform: wifi_platform,
                    },
                });
            }
        }
    } else {
        None
    };

    set_wifi_rx(lease, &membership, true);
    let initial_channel = lower_wifi_channel(config.initial_channel);
    let selected = {
        let (mut channel, phy) = clocked.channel_hal_with_attachment(platform, lease);
        select_concurrent_wifi_channel::<D, _, _>(
            timer,
            phy,
            initial_channel.channel_or_frequency,
            initial_channel.cbw,
            &mut channel,
            &mut observer,
        )
        .await
    };
    if let Err(error) = selected {
        if matches!(error, ConcurrentWifiChannelError::Rejected(_)) {
            // Nothing started; restore the receive path the joined frontier
            // reports.
            set_wifi_rx(lease, &membership, false);
        }
        return Err(WifiColdStartFailure {
            error: WifiColdStartError::InitialChannel(error),
            frontier: WifiColdStartFrontier::Joined {
                clocked,
                membership,
                platform: wifi_platform,
            },
        });
    }

    let tx_power = match lease.attachment().phy_state() {
        Ok(state) => state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(config.maximum_tx_power_quarter_dbm),
        Err(error) => {
            set_wifi_rx(lease, &membership, false);
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::InitialChannel(ConcurrentWifiChannelError::Rejected(
                    error,
                )),
                frontier: WifiColdStartFrontier::Joined {
                    clocked,
                    membership,
                    platform: wifi_platform,
                },
            });
        }
    };
    Ok(WifiColdStart {
        clocked,
        membership,
        platform: wifi_platform,
        report: WifiColdStartReport {
            initial_channel: config.initial_channel,
            initial_tracking,
            tx_power,
        },
    })
}
