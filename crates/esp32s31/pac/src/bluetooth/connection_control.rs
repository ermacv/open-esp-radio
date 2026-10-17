//! Connection opcode inspection policy for the exclusive task owner.

use crate::{BluetoothTaskRegisters, device_fence};

impl BluetoothTaskRegisters {
    /// Leave connection control-PDU interpretation to the software LL owner.
    ///
    /// Software CCM publishes ciphertext to the radio. Its first payload byte
    /// can equal the default abort opcode, `LL_TERMINATE_IND`, without being a
    /// termination. Clear the reviewed link-state control field before RX/RUN
    /// publication so the authenticated software decoder decides the opcode.
    /// The caller must hold an idle, powered task epoch with no active RUN.
    pub fn prepare_software_connection_packet_control(&mut self) {
        crate::generated::clear_ble_connection_link_state_control(
            &self.bluetooth.btmac_ble_phy_init,
        );
        device_fence();
    }

    /// Restore the initialization policy before publishing another RX role.
    ///
    /// The caller must hold an idle, powered task epoch with no active RUN.
    pub fn restore_default_connection_packet_control(&mut self) {
        crate::generated::set_ble_connection_link_state_control(&self.bluetooth.btmac_ble_phy_init);
        device_fence();
    }
}
