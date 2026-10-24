//! MAC receive-interface addresses.

use oer_mmio::{Index, Placement};

use crate::{MacInterface, wifi_mac_interface_address::WifiMacInterfaceAddress};

impl MacInterface {
    /// The interface's element of the four-entry interface register arrays.
    pub const fn index(self) -> Index<4> {
        match self {
            Self::Station => Index::constant::<0>(),
            Self::AccessPoint => Index::constant::<1>(),
            Self::Context2 => Index::constant::<2>(),
            Self::Context3 => Index::constant::<3>(),
        }
    }
}

/// The three hardware operations of one interface-address publication.
trait InterfaceAddressPort {
    /// The full-word store of the address's low four bytes.
    fn store_low(&mut self, index: Index<4>, word: u32);
    /// The zero-based store of the address's high two bytes.
    fn store_high(&mut self, index: Index<4>, bytes: u16);
    /// The fresh read-modify-write that sets the receive-policy enable.
    fn enable_receive_policy(&mut self, index: Index<4>);
}

impl<P: Placement> InterfaceAddressPort for WifiMacInterfaceAddress<P> {
    fn store_low(&mut self, index: Index<4>, word: u32) {
        self.mac_interface_address_low(index, word);
    }

    fn store_high(&mut self, index: Index<4>, bytes: u16) {
        self.mac_interface_address_high(index, bytes);
    }

    fn enable_receive_policy(&mut self, index: Index<4>) {
        self.enable_mac_interface_receive_policy(index);
    }
}

/// `hal_mac_set_addr`'s operations, in its order.
fn program<Port: InterfaceAddressPort>(port: &mut Port, interface: MacInterface, address: [u8; 6]) {
    let index = interface.index();
    port.store_low(
        index,
        u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
    );
    port.store_high(index, u16::from_le_bytes([address[4], address[5]]));
    port.enable_receive_policy(index);
}

impl<P: Placement> WifiMacInterfaceAddress<P> {
    /// Publish one MAC address and enable it for receive-policy matching.
    ///
    /// SOURCE(esp32s31,esp32c5): complete pinned
    /// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
    ///
    /// The complete leaf performs three ordered hardware operations. In
    /// particular, the enable edge is a fresh-read RMW and must not be folded
    /// into the preceding full-word high-address store.
    #[inline]
    pub fn program_receive_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        program(self, interface, address);
    }
}

#[cfg(test)]
mod tests;
