//! Generated-PAC ownership for the direct pre-COEX tail of `hal_init`.

#![forbid(unsafe_code)]

use crate::{MacInterruptMask, MacInterruptSetup, WifiRadioRegisters};

/// The RTC calibration field after the vendor OS-interface conversion.
/// It carries only a register value, not evidence that a clock was measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRtcCalibration(u32);

impl MacRtcCalibration {
    pub const ZERO: Self = Self(0);

    /// Preserve the exact truncation performed by `hal_timer_update_by_rtc`.
    pub const fn from_osi_value(value: u32) -> Self {
        Self(value & 0x0003_ffff)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl WifiRadioRegisters {
    /// Apply the complete direct tail through `hal_timer_update_by_rtc`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0xcc..0x12a`, the complete hardware-beacon reload leaf, and
    /// complete `libpp.a[hal_tsf.o]::hal_timer_update_by_rtc`.
    ///
    /// The typed calibration fits the complete vendor field before MMIO begins.
    pub fn initialize_mac_hal_tail(
        &mut self,
        interrupts: &mut MacInterruptSetup,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacRtcCalibration,
    ) {
        crate::wifi::mac::interrupt::publish_mac_interrupt_mask(&interrupts.peripheral, event_mask);

        // This is deliberately a repeated edge: mac_txrx_init already set the
        // same bit, and complete hal_init samples and sets it again here.
        self.peripherals
            .wifi_mac
            .wifi_mac_txrx_prefix
            .feature_edges()
            .modify(|_, w| w.bit_28_opaque().set_bit());

        let csi = self.peripherals.wifi_mac.wifi_mac_rx_csi_control.control();
        // Keep the fresh-read byte replacements separate and in blob order.
        csi.modify(|_, w| w.bits_0_7_opaque().initialized());
        csi.modify(|_, w| w.bits_8_15_opaque().initialized());

        self.peripherals
            .wifi_mac
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.bit_27_opaque().set_bit());

        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.control()
            .modify(|_, w| w.lp_clock_sync_enable().set_bit());
        rtc.slow_clock_calibration()
            .modify(|_, w| w.value().set(slow_clock_calibration.value()));
    }
}
