//! Restricted BLE Controller device-address publication.
//!
//! The generated PAC describes three positional six-octet slots. Reviewed
//! callers identify slot zero as the public-device address and slot one as the
//! random-device address. This facade fixes those selections so no upper layer
//! can supply a positional slot or reach the still-unnamed third slot.

#![deny(unsafe_code)]

use crate::{BluetoothTaskRegisters, device_fence};

const PUBLIC_DEVICE_ADDRESS_SLOT: usize = 0;
const RANDOM_DEVICE_ADDRESS_SLOT: usize = 1;

fn program_controller_address_slot(
    registers: &crate::svd::BleControllerAddressSlots,
    slot: usize,
    wire_octets: [u8; 6],
) {
    let [octet_0, octet_1, octet_2, octet_3, octet_4, octet_5] = wire_octets;
    crate::svd::zero_based_field_write::ble_controller_address_slot_low(
        registers,
        slot,
        u32::from_le_bytes([octet_0, octet_1, octet_2, octet_3]),
    );
    crate::svd::zero_based_field_write::ble_controller_address_slot_high(
        registers,
        slot,
        u16::from_le_bytes([octet_4, octet_5]),
    );
    device_fence();
}

impl BluetoothTaskRegisters {
    /// Publish the public-device address to its fixed Controller slot.
    ///
    /// `wire_octets` are in Bluetooth LE/HCI order: least-significant address
    /// octet first. The generated PAC owns the complete field writes; this
    /// facade owns only byte packing, the reviewed low-before-high order and
    /// the fixed public-slot selection.
    ///
    /// The owning lifecycle must retain a powered BLE Controller epoch and
    /// ensure that no active advertising, scanning or connection operation
    /// can consume the address slot while the pair is being replaced.
    #[doc(hidden)]
    pub fn program_bluetooth_public_device_address(&mut self, wire_octets: [u8; 6]) {
        program_controller_address_slot(
            &self.bluetooth.ble_controller_address_slots,
            PUBLIC_DEVICE_ADDRESS_SLOT,
            wire_octets,
        );
    }

    /// Publish the random-device address to its fixed Controller slot.
    ///
    /// `wire_octets` are in Bluetooth LE/HCI order: least-significant address
    /// octet first. The generated PAC owns the complete field writes; this
    /// facade owns only byte packing, the reviewed low-before-high order and
    /// the fixed random-slot selection.
    ///
    /// The owning lifecycle must retain a powered BLE Controller epoch and
    /// ensure that no active advertising, scanning or connection operation
    /// can consume the address slot while the pair is being replaced.
    #[doc(hidden)]
    pub fn program_bluetooth_random_device_address(&mut self, wire_octets: [u8; 6]) {
        program_controller_address_slot(
            &self.bluetooth.ble_controller_address_slots,
            RANDOM_DEVICE_ADDRESS_SLOT,
            wire_octets,
        );
    }
}
