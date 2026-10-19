//! Generated-PAC ownership for the complete MAC enable transaction.

#![forbid(unsafe_code)]

use crate::{MacInterruptMask, MacInterruptSetup, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Enable the shared MAC gates and publish the enabled-event bitmap.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::hal_enable_mac`, size `0x18`.
    /// It clears all four disable gates in one fresh-read RMW, then stores its
    /// complete argument into the interrupt-enable register.
    pub fn enable_mac_with_interrupt_mask(
        &mut self,
        interrupts: &mut MacInterruptSetup,
        event_mask: MacInterruptMask,
    ) {
        crate::generated::enable_wifi_mac_core(&self.peripherals.wifi_mac.wifi_mac_core_enable);
        crate::wifi::mac::interrupt::publish_mac_interrupt_mask(&interrupts.peripheral, event_mask);
    }
}
