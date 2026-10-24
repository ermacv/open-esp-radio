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
        let index = interface.index();
        self.mac_interface_address_low(
            index,
            u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
        );
        self.mac_interface_address_high(index, u16::from_le_bytes([address[4], address[5]]));
        self.enable_mac_interface_receive_policy(index);
    }
}

#[cfg(test)]
mod tests;
