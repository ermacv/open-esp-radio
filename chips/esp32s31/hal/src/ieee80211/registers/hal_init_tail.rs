//! The direct pre-COEX tail of `hal_init`.

use oer_esp32s31_pac::{
    MacInterruptMask,
    wifi_mac_rx_csi_control::control::{Bits0_7Opaque, Bits8_15Opaque},
};
use oer_mmio::Bits;

use super::{MacInterruptSetup, MacPlacement, WifiRadioRegisters, WifiRxDmaRegisters, interrupt};

/// The RTC calibration field after the vendor OS-interface conversion.
/// It carries only a register value, not evidence that a clock was measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRtcCalibration(Bits<18>);

impl MacRtcCalibration {
    pub const ZERO: Self = Self(Bits::constant::<0>());

    /// Preserve the exact truncation performed by `hal_timer_update_by_rtc`
    /// to the eighteen-bit field.
    pub const fn from_osi_value(value: u32) -> Self {
        Self(Bits::truncate(value))
    }

    pub const fn value(self) -> u32 {
        self.0.get()
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply the complete direct tail through `hal_timer_update_by_rtc`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0xcc..0x12a`, the complete hardware-beacon reload leaf, and
    /// complete `libpp.a[hal_tsf.o]::hal_timer_update_by_rtc`
    /// (BLOB_LIBPP_HAL_INIT_TAIL).
    ///
    /// The typed calibration fits the complete vendor field before MMIO begins.
    pub fn initialize_mac_hal_tail(
        &mut self,
        rx_dma: &mut WifiRxDmaRegisters<M>,
        interrupts: &mut MacInterruptSetup<M>,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacRtcCalibration,
    ) {
        interrupt::publish_mac_interrupt_mask(&mut interrupts.mac, event_mask);

        // This is deliberately a repeated edge: mac_txrx_init already set the
        // same bit, and complete hal_init samples and sets it again here.
        self.mac.wifi_mac_txrx_prefix.modify_feature_edges(|w| {
            w.set_bit_28_opaque(true);
        });

        // Keep the fresh-read byte replacements separate and in blob order.
        let csi = &mut self.mac.wifi_mac_rx_csi_control;
        csi.modify_control(|w| {
            w.set_bits_0_7_opaque(Bits0_7Opaque::Initialized);
        });
        csi.modify_control(|w| {
            w.set_bits_8_15_opaque(Bits8_15Opaque::Initialized);
        });

        rx_dma.dma.modify_rx_control(|w| {
            w.set_bit_27_opaque(true);
        });

        let rtc = &mut self.mac.wifi_mac_rtc_timer_update;
        rtc.modify_control(|w| {
            w.set_lp_clock_sync_enable(true);
        });
        rtc.modify_slow_clock_calibration(|w| {
            w.set_value(slow_clock_calibration.0);
        });
    }
}
