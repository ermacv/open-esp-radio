//! Connection opcode inspection policy for the exclusive task owner.

use crate::bluetooth::registers::{BluetoothPlacements, BluetoothTaskRegisters, device_fence};

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Leave connection control-PDU interpretation to the software LL owner.
    ///
    /// Software CCM publishes ciphertext to the radio. Its first payload byte
    /// can equal the default abort opcode, `LL_TERMINATE_IND`, without being a
    /// termination. Clear the reviewed link-state control field before RX/RUN
    /// publication so the authenticated software decoder decides the opcode.
    /// The caller must hold an idle, powered task epoch with no active RUN.
    pub fn prepare_software_connection_packet_control(&mut self) {
        self.bluetooth
            .btmac_ble_phy_init
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(false);
            });
        device_fence();
    }

    /// Restore the initialization policy before publishing another RX role.
    ///
    /// The caller must hold an idle, powered task epoch with no active RUN.
    pub fn restore_default_connection_packet_control(&mut self) {
        self.bluetooth
            .btmac_ble_phy_init
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(true);
            });
        device_fence();
    }
}
