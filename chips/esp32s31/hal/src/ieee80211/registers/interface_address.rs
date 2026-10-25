//! The shared MAC receive-interface address block.

use oer_esp32s31_pac::MacInterface;

use super::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// The shared library's transaction
    /// `WifiMacInterfaceAddress::program_receive_interface_address`.
    pub fn program_receive_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.interface
            .program_receive_interface_address(interface, address);
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
