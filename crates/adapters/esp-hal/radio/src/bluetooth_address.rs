//! ESP32-S31 interface-address policy above the safe eFuse accessor.

use oer_bluetooth_hci::BluetoothPublicDeviceAddress;

pub(crate) fn bluetooth_public_address_from_base(
    mut base: [u8; 6],
) -> BluetoothPublicDeviceAddress {
    // The pinned S31 MAC policy selects two universal addresses: Wi-Fi STA is
    // the base and Bluetooth is the next final-octet value.
    base[5] = base[5].wrapping_add(1);
    BluetoothPublicDeviceAddress::from_canonical_bytes(base)
}

#[cfg(test)]
mod tests;
