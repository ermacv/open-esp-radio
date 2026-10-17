//! MAC receive-interface addresses.

use crate::{MacInterface, svd};

/// The register block of the four interface-address pairs.
pub type InterfaceAddressRegisters = svd::WifiMacInterfaceAddress;

/// Publish one MAC address and enable it for receive-policy matching.
///
/// SOURCE(esp32s31,esp32c5): complete pinned
/// `libpp.a[hal_mac.o]::hal_mac_set_addr`.
///
/// The complete leaf performs three ordered hardware operations. In
/// particular, the enable edge is a fresh-read RMW and must not be folded
/// into the preceding full-word high-address store.
#[inline]
pub fn program_receive_interface_address(
    addresses: &InterfaceAddressRegisters,
    interface: MacInterface,
    address: [u8; 6],
) {
    let interface = interface.bits() as usize;
    svd::zero_based_field_write::mac_interface_address_low(
        addresses,
        interface,
        u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
    );
    svd::zero_based_field_write::mac_interface_address_high(
        addresses,
        interface,
        u16::from_le_bytes([address[4], address[5]]),
    );
    crate::generated::enable_mac_interface_receive_policy(addresses, interface);
}
