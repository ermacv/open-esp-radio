//! Register-local cold MAC preparation and runtime start.

#![forbid(unsafe_code)]

use crate::{MacInterruptMask, MacInterruptSetup, WifiRadioRegisters, device_fence};

impl WifiRadioRegisters {
    /// Hold all common MAC gates while the cold configuration has no RX ring.
    /// SOURCE: complete `hal_disable_mac`, BLOB_LIBPP_HAL_ENABLE_MAC.
    pub fn hold_mac_runtime(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_core_enable
            .control()
            .modify(|_, w| w.bits_4_7_opaque().disabled());
        device_fence();
        // Initialize the cold runtime image while the core is held, then
        // retain its no-power-save stop through BASE and walker publication.
        crate::svd::zero_register_write::open_mac_control_cold(
            &self.peripherals.wifi_mac.wifi_mac_control,
        );
        self.request_mac_channel_stop_without_power_save();
    }

    /// Publish the cold interrupt policy without opening the MAC core.
    /// SOURCE: interrupt suffix of complete `hal_enable_mac`.
    pub fn prepare_mac_interrupts(
        &mut self,
        interrupts: &mut MacInterruptSetup,
        event_mask: MacInterruptMask,
    ) {
        crate::wifi::mac::interrupt::publish_mac_interrupt_mask(&interrupts.peripheral, event_mask);
    }

    /// Clock the prepared MAC core without releasing its runtime stop.
    /// SOURCE: BLOB_LIBPP_HAL_ENABLE_MAC. The DMA walker needs this core edge
    /// before its own enable; MAC RX remains held until the later resume.
    pub fn prepare_mac_runtime(&mut self) {
        crate::generated::enable_wifi_mac_core(&self.peripherals.wifi_mac.wifi_mac_core_enable);
        device_fence();
    }

    /// Whether every common MAC disable gate is still asserted.
    pub fn mac_core_gates_closed(&self) -> bool {
        self.peripherals
            .wifi_mac
            .wifi_mac_core_enable
            .control()
            .read()
            .bits_4_7_opaque()
            .is_disabled()
    }
}
