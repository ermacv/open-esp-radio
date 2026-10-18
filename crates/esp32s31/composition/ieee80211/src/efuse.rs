//! The interface addresses the chip was programmed with.
//!
//! Watchdog budgets and the initial channel stay explicit: they are board
//! decisions, and the composition supplies no qualified defaults.

use esp_hal::efuse::{self, InterfaceMacAddress};
use oer_ieee80211_mac::channel::WifiChannel;
use oer_radio::wifi::{WifiMacAddress, WifiMacAddressError};

use crate::{RadioConfig, WatchdogConfig};

/// An eFuse interface address that cannot address a Wi-Fi interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EfuseMacError {
    Station(WifiMacAddressError),
    AccessPoint(WifiMacAddressError),
}

fn efuse_mac(interface: InterfaceMacAddress) -> Result<WifiMacAddress, WifiMacAddressError> {
    let mut address = [0; 6];
    address.copy_from_slice(efuse::interface_mac_address(interface).as_bytes());
    WifiMacAddress::new(address)
}

impl RadioConfig {
    /// [`RadioConfig::new`] with the station and access-point addresses the
    /// chip's eFuse holds.
    ///
    /// # Errors
    ///
    /// When an eFuse address is not a unicast address.
    pub fn from_efuse(
        watchdog: &'static WatchdogConfig,
        initial_channel: WifiChannel,
    ) -> Result<Self, EfuseMacError> {
        let station = efuse_mac(InterfaceMacAddress::Station).map_err(EfuseMacError::Station)?;
        let access_point =
            efuse_mac(InterfaceMacAddress::AccessPoint).map_err(EfuseMacError::AccessPoint)?;
        Ok(Self::new(watchdog, station, access_point, initial_channel))
    }
}
