//! Register-local operations of the cold MAC handshake.

use super::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Set the cold-start request bit.
    pub fn request_mac_cold_start(&mut self) {
        self.mac
            .wifi_mac_cold_handshake
            .modify_cold_handshake_control(|w| {
                w.set_request(true);
            });
    }

    /// Sample the cold-start ready field once.
    pub fn sample_mac_cold_start_ready(&self) -> bool {
        self.mac
            .wifi_mac_cold_handshake
            .read_cold_handshake_control()
            .ready()
    }
}
