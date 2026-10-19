//! Register-local PAC operations for the cold MAC handshake.

#![forbid(unsafe_code)]

use crate::{MacInterruptMask, MacInterruptSetup, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Set the cold-start request bit.
    pub fn request_mac_cold_start(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_cold_handshake
            .control()
            .modify(|_, w| w.request().set_bit());
    }

    /// Sample the cold-start ready field once.
    pub fn sample_mac_cold_start_ready(&self) -> bool {
        self.peripherals
            .wifi_mac
            .wifi_mac_cold_handshake
            .control()
            .read()
            .ready()
            .bit_is_set()
    }
}

impl MacInterruptSetup {
    /// Mask every MAC interrupt source.
    pub fn mask_all_mac_interrupts(&mut self) {
        crate::wifi::mac::interrupt::publish_mac_interrupt_mask(
            &self.peripheral,
            MacInterruptMask::NONE,
        );
    }

    /// Acknowledge every pending MAC interrupt source.
    pub fn clear_all_mac_interrupts(&mut self) {
        crate::generated::mac_interrupt_clear(
            &self.peripheral,
            crate::generated::MacInterruptClearImage::new(u32::MAX),
        );
    }
}
