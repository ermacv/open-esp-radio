//! Executor-neutral transition from the joined Wi-Fi client to cold MAC ownership.

use crate::cold_start::{WifiColdStart, WifiColdStartReport};

use oer_esp32s31_hal::{ieee80211::client::WifiClocked, shared_radio::SharedRadioLease};
use oer_esp32s31_phy::{PhyTxTargetPowerProfile, wifi_client::WifiPhyMembership};

use oer_esp32s31_ieee80211_mac::init::{
    MacCoexPtiSource, MacColdStartError, MacColdStartOutcome, MacDelayEntropy,
    MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
};

use oer_ieee80211_softmac::WifiMacAddress;

/// Platform operations needed to join the calibrated PHY power table to the
/// finite MAC initializer.
///
/// The installation method is deliberately semantic: MAC code never borrows
/// the PHY parameter arena and never depends on an ESP-HAL singleton type.
pub trait WifiMacPlatform:
    MacCoexPtiSource + MacDelayEntropy + MacSlowClockCalibrationSource + MacTxPowerSource
{
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile);
}

/// Role-neutral inputs for the common Wi-Fi MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiMacStartConfig {
    handshake_sample_limit: u32,
    station_address: WifiMacAddress,
    access_point_address: WifiMacAddress,
}

impl WifiMacStartConfig {
    pub const fn new(
        handshake_sample_limit: u32,
        station_address: WifiMacAddress,
        access_point_address: WifiMacAddress,
    ) -> Self {
        Self {
            handshake_sample_limit,
            station_address,
            access_point_address,
        }
    }

    pub const fn handshake_sample_limit(self) -> u32 {
        self.handshake_sample_limit
    }

    pub const fn station_address(self) -> WifiMacAddress {
        self.station_address
    }

    pub const fn access_point_address(self) -> WifiMacAddress {
        self.access_point_address
    }
}

/// Reports from the PHY and common MAC transitions kept with the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiMacStartReport {
    pub wifi: WifiColdStartReport,
    pub mac: MacColdStartOutcome,
}

/// Clocked Wi-Fi client after common MAC initialization but before
/// role-specific RX, DMA and interrupt policy is activated.
pub struct WifiMacReady<W> {
    pub(crate) clocked: WifiClocked,
    pub(crate) membership: WifiPhyMembership,
    pub(crate) platform: W,
    report: WifiMacStartReport,
}

impl<W> WifiMacReady<W> {
    pub const fn report(&self) -> WifiMacStartReport {
        self.report
    }
}

/// Failed MAC transition retaining the joined Wi-Fi client.
#[must_use = "a failed MAC start still owns the joined Wi-Fi client"]
pub struct WifiMacStartFailure<W> {
    pub error: MacColdStartError,
    cold: WifiColdStart<W>,
}

impl<W> WifiMacStartFailure<W> {
    /// Recover the joined Wi-Fi client and its PHY report.
    pub fn into_parts(
        self,
    ) -> (
        WifiClocked,
        WifiPhyMembership,
        W,
        WifiColdStartReport,
        MacColdStartError,
    ) {
        let WifiColdStart {
            clocked,
            membership,
            platform,
            report,
            ..
        } = self.cold;
        (clocked, membership, platform, report, self.error)
    }
}

/// Perform the common MAC transition exactly once after the Wi-Fi client
/// joined the shared PHY domain. The shared registers of the cold MAC
/// transaction are borrowed from the arbiter lease.
pub fn start_esp32s31_wifi_mac<W, T>(
    mut cold: WifiColdStart<W>,
    lease: &mut SharedRadioLease<'_, T>,
    config: WifiMacStartConfig,
) -> Result<WifiMacReady<W>, WifiMacStartFailure<W>>
where
    W: WifiMacPlatform,
{
    let mac = {
        let WifiColdStart {
            clocked,
            platform,
            report,
            ..
        } = &mut cold;
        platform.install_phy_tx_power_profile(report.tx_power);
        let mut mac = clocked.cold_mac_hal(lease);
        initialize_wifi_mac(
            platform,
            &mut mac,
            oer_esp32s31_ieee80211_mac::init::MacColdStartConfig {
                handshake_sample_limit: config.handshake_sample_limit,
                station_address: config.station_address.bytes(),
                access_point_address: config.access_point_address.bytes(),
            },
        )
    };
    let mac = match mac {
        Ok(mac) => mac,
        Err(error) => return Err(WifiMacStartFailure { error, cold }),
    };
    let WifiColdStart {
        clocked,
        membership,
        platform,
        report,
        ..
    } = cold;
    Ok(WifiMacReady {
        clocked,
        membership,
        platform,
        report: WifiMacStartReport { wifi: report, mac },
    })
}
