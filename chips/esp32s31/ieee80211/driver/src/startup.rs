//! Role-neutral ESP32-S31 Wi-Fi initialization as a client of the radio
//! system.
//!
//! The Wi-Fi bring-up and common MAC startup happen once per Wi-Fi epoch,
//! through the client's membership cell: `join` powers and clocks the
//! client and enters it into the shared PHY domain, and `track` runs the
//! initial tracking, the RX enable, the initial channel and cold MAC
//! preparation; the actor does the shared steps of both. Station, access-point,
//! scan and monitor owners are materialized later from the returned stopped
//! frontier; startup therefore cannot accidentally lock the radio into the
//! first role an application happens to use.

use oer_esp32s31_hal::ieee80211::init::{
    MacColdStartConfig, MacDelayEntropy, MacSlowClockCalibrationSource,
};
use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiCold},
    root::WifiPartition,
};
use oer_esp32s31_phy::PhyTxTargetPowerProfile;
use oer_esp32s31_radio_client::{
    Failed, Failure, Joined, Member, MembershipCell, PhyPreparation, RfEpoch, TrackedMember,
    Untracked,
    wifi::{TrackReport, Wifi, WifiJoin, WifiJoined, WifiReturned, WifiTrack, WifiTracked},
};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::channel::WifiChannel;
use oer_time::Duration;

use crate::runtime::{WifiStopped, enter_esp32s31_wifi_runtime};
use oer_radio_regulatory::{Permitted, TxCeiling};

/// The values Wi-Fi's own platform gives the MAC bring-up, and where it keeps
/// the calibrated TX power profile for its TX vectors.
///
/// The coexistence priorities and the PHY power table stay with the radio
/// system, which answers the MAC cold start's queries for them.
pub trait WifiMacPlatform: MacDelayEntropy + MacSlowClockCalibrationSource {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile);
}

/// Application-selected inputs for one Wi-Fi bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiColdStartConfig {
    /// The channel the regulatory domain permits.
    pub initial_channel: Permitted<WifiChannel>,
    /// The most any transmission radiates: the domain's, the board's and the
    /// chip's limits together.
    pub tx_ceiling: TxCeiling,
}

impl WifiColdStartConfig {
    pub const fn new(initial_channel: Permitted<WifiChannel>, tx_ceiling: TxCeiling) -> Self {
        Self {
            initial_channel,
            tx_ceiling,
        }
    }
}

/// Role-neutral inputs for the common Wi-Fi MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiMacStartConfig {
    handshake_deadline: Duration,
    station_address: IndividualAddress,
    access_point_address: IndividualAddress,
}

impl WifiMacStartConfig {
    /// Retain two nonzero interface addresses before starting the hardware.
    pub const fn new(
        handshake_deadline: Duration,
        station_address: IndividualAddress,
        access_point_address: IndividualAddress,
    ) -> Result<Self, oer_ieee80211_wire::address::IndividualAddressError> {
        if matches!(station_address.get(), [0, 0, 0, 0, 0, 0])
            || matches!(access_point_address.get(), [0, 0, 0, 0, 0, 0])
        {
            return Err(oer_ieee80211_wire::address::IndividualAddressError::Unspecified);
        }
        Ok(Self {
            handshake_deadline,
            station_address,
            access_point_address,
        })
    }

    pub const fn handshake_deadline(self) -> Duration {
        self.handshake_deadline
    }

    pub const fn station_address(self) -> IndividualAddress {
        self.station_address
    }

    pub const fn access_point_address(self) -> IndividualAddress {
        self.access_point_address
    }
}

/// Inputs for the one common Wi-Fi PHY/MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioStartConfig {
    wifi: WifiColdStartConfig,
    mac: WifiMacStartConfig,
}

impl RadioStartConfig {
    pub const fn new(wifi: WifiColdStartConfig, mac: WifiMacStartConfig) -> Self {
        Self { wifi, mac }
    }
}

/// What the bring-up observed, kept with the stopped owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WifiStartReport {
    pub initial_channel: WifiChannel,
    /// The RF epoch Wi-Fi joined.
    pub epoch: RfEpoch,
    /// How the join prepared the PHY domain.
    pub phy: PhyPreparation,
    /// The initial tracking, TX power profile and cold MAC preparation.
    pub track: TrackReport,
}

/// A bring-up that did not complete, with what the radio system returned.
#[allow(
    clippy::large_enum_variant,
    reason = "no-alloc start failure retains the exact recoverable Wi-Fi frontier"
)]
pub enum RadioStartFailure<W> {
    /// `join` failed; the partition comes back at the stage it reached.
    Join {
        failed: Failed<WifiReturned>,
        platform: W,
    },
    /// `track` failed; the client is still an untracked member, which must
    /// leave through its cell.
    Track {
        failed: Failed<(Member<Wifi, Untracked>, WifiClocked)>,
        platform: W,
    },
}

impl<W> RadioStartFailure<W> {
    /// Whether the failure poisoned the shared domain. This observation does
    /// not release ownership or choose a platform response.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        let cause = match self {
            Self::Join { failed, .. } => failed.cause,
            Self::Track { failed, .. } => failed.cause,
        };
        matches!(cause, Failure::Poisoned(_))
    }
}

/// Bring Wi-Fi up through its membership cell and perform the common MAC
/// initialization without choosing a Wi-Fi role. Answered requests return
/// the cell with their outcome. A lost mandatory response quarantines the
/// seat and returns the retained platform in `RequestQuarantine`.
///
/// # Cancellation
///
/// Dropping the future loses no answer: it stays in the cell, whose token
/// this future holds while a request is posted.
pub async fn start_esp32s31_radio<W: WifiMacPlatform>(
    cell: MembershipCell<Wifi>,
    partition: WifiPartition,
    mut platform: W,
    config: RadioStartConfig,
) -> Result<
    (
        MembershipCell<Wifi>,
        Result<WifiStopped<W>, RadioStartFailure<W>>,
    ),
    oer_memory::RequestQuarantine<Option<W>>,
> {
    let (cell, joined) = match cell
        .join(WifiJoin {
            wifi: WifiCold::from_partition(partition),
        })
        .await
    {
        Ok(answer) => answer,
        Err(reason) => return Err(oer_memory::RequestQuarantine::new(reason, Some(platform))),
    };
    let Joined {
        member,
        epoch,
        payload: WifiJoined { wifi, phy },
    } = match joined {
        Ok(joined) => joined,
        Err(failed) => return Ok((cell, Err(RadioStartFailure::Join { failed, platform }))),
    };
    let request = WifiTrack {
        wifi,
        channel: config.wifi.initial_channel,
        tx_ceiling: config.wifi.tx_ceiling,
        mac: MacColdStartConfig {
            handshake_deadline: config.mac.handshake_deadline,
            station_address: config.mac.station_address.get(),
            access_point_address: config.mac.access_point_address.get(),
        },
        slow_clock: platform.mac_slow_clock_calibration(),
        delay_random: platform.mac_delay_random(),
    };
    let (cell, tracked) = match cell.track(member, request).await {
        Ok(answer) => answer,
        Err(reason) => return Err(oer_memory::RequestQuarantine::new(reason, Some(platform))),
    };
    let TrackedMember {
        member,
        payload: WifiTracked { wifi, report },
    } = match tracked {
        Ok(tracked) => tracked,
        Err(failed) => return Ok((cell, Err(RadioStartFailure::Track { failed, platform }))),
    };
    platform.install_phy_tx_power_profile(report.tx_power);
    let report = WifiStartReport {
        initial_channel: config.wifi.initial_channel.channel(),
        epoch,
        phy,
        track: report,
    };
    Ok((
        cell,
        Ok(enter_esp32s31_wifi_runtime(wifi, member, platform, report)),
    ))
}
