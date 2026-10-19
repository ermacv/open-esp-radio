//! Ownership boundary for MAC interface-address publication.

use oer_esp32s31_hal::ieee80211::mac::{MacInterface, WifiMacColdHal};

/// Finite hardware capability needed to publish the two cold-path addresses.
pub trait MacInterfaceAddressHardware {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]);
}

impl MacInterfaceAddressHardware for WifiMacColdHal<'_> {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        WifiMacColdHal::program_interface_address(self, interface, address);
    }
}

/// Publish the STA and AP receive addresses required by the cold MAC path.
pub(crate) fn program_cold_receive_addresses<H: MacInterfaceAddressHardware>(
    hardware: &mut H,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) {
    hardware.program_interface_address(MacInterface::Station, station_address);
    hardware.program_interface_address(MacInterface::AccessPoint, access_point_address);
}
