//! ESP32-S31 ownership of the shared MAC receive-interface address block.

#![forbid(unsafe_code)]

use crate::{MacInterface, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// The shared transaction of
    /// [`oer_ieee80211_pac::interface_address::program_receive_interface_address`].
    pub fn program_receive_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        oer_ieee80211_pac::interface_address::program_receive_interface_address(
            &self.peripherals.wifi_mac.wifi_mac_interface_address,
            interface,
            address,
        );
    }

    /// Publish the STA and AP interface addresses used by the open cold path.
    pub fn program_sta_ap_receive_addresses(
        &mut self,
        station_address: [u8; 6],
        access_point_address: [u8; 6],
    ) {
        self.program_receive_interface_address(MacInterface::Station, station_address);
        self.program_receive_interface_address(MacInterface::AccessPoint, access_point_address);
    }
}
