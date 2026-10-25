//! The MAC receive-interface address pairs.

use oer_esp32s31_pac::wifi_mac::WifiMacInterfaceAddress;
use oer_mmio::Placement;

use super::{MacInterface, MacPlacement, WifiRadioRegisters, bits};

/// `hal_mac_set_addr`'s three hardware operations, in its order.
///
/// SOURCE(esp32s31,esp32c5): complete pinned
/// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
///
/// The complete leaf performs three ordered hardware operations: the
/// full-word store of the address's low four bytes, the zero-based store of
/// its high two bytes and the fresh-read RMW that sets the receive-policy
/// enable. In particular, the enable edge must not be folded into the
/// preceding high-address store.
fn program<P: Placement>(
    block: &mut WifiMacInterfaceAddress<P>,
    interface: MacInterface,
    address: [u8; 6],
) {
    let index = interface.index();
    let [a0, a1, a2, a3, a4, a5] = address;
    block.modify_address_low(index, |w| {
        w.set_bytes_0_3(u32::from_le_bytes([a0, a1, a2, a3]));
    });
    block.modify_address_high(index, |w| {
        w.set_bytes_4_5(u16::from_le_bytes([a4, a5]))
            .set_rx_policy_enable(false)
            .set_bits_17_31_opaque(bits(0));
    });
    block.modify_address_high(index, |w| {
        w.set_rx_policy_enable(true);
    });
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// SOURCE(esp32s31,esp32c5): complete pinned
    /// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
    pub fn program_receive_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        program(&mut self.interface, interface, address);
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

#[cfg(test)]
mod tests;
