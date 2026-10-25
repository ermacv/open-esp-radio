//! The direct pre-COEX tail of `hal_init`.

use oer_esp32s31_pac::wifi_mac::rx_csi_control::{Bits0_7Opaque, Bits8_15Opaque};

use super::{
    MacPlacement, MacSlowClockCalibrationValue, WifiRadioRegisters, WifiRxDmaRegisters, bits,
};

/// The RTC calibration field after the vendor OS-interface conversion.
/// It carries only a register value, not evidence that a clock was measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRtcCalibration(MacSlowClockCalibrationValue);

impl MacRtcCalibration {
    pub const ZERO: Self = Self(match MacSlowClockCalibrationValue::new(0) {
        Some(zero) => zero,
        None => unreachable!(),
    });

    /// Preserve the exact truncation performed by `hal_timer_update_by_rtc`.
    pub const fn from_osi_value(value: u32) -> Self {
        match MacSlowClockCalibrationValue::new(value & MacSlowClockCalibrationValue::MAX) {
            Some(fitted) => Self(fitted),
            None => unreachable!(),
        }
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
    /// complete `libpp.a[hal_tsf.o]::hal_timer_update_by_rtc`.
    ///
    /// The typed calibration fits the complete vendor field before MMIO begins.
    /// The tail's first edge, the vendor's publication of the station
    /// interrupt mask, is not this owner's: the MAC's interrupt banks belong
    /// to the radio interrupt handler, which publishes the mask when it
    /// activates an epoch.
    pub fn initialize_mac_hal_tail(
        &mut self,
        rx_dma: &mut WifiRxDmaRegisters<M>,
        slow_clock_calibration: MacRtcCalibration,
    ) {
        // This is deliberately a repeated edge: mac_txrx_init already set the
        // same bit, and complete hal_init samples and sets it again here.
        self.mac
            .wifi_mac_txrx_prefix
            .modify_txrx_prefix_feature_edges(|w| {
                w.set_bit_28_opaque(true);
            });

        let csi = &mut self.mac.wifi_mac_rx_csi_control;
        // Keep the fresh-read byte replacements separate and in blob order.
        csi.modify_rx_csi_control(|w| {
            w.set_bits_0_7_opaque(Bits0_7Opaque::Initialized);
        });
        csi.modify_rx_csi_control(|w| {
            w.set_bits_8_15_opaque(Bits8_15Opaque::Initialized);
        });

        rx_dma.set_mac_rx_control_bit_27();

        let rtc = &mut self.mac.wifi_mac_rtc_timer_update;
        rtc.modify_rtc_timer_update_control(|w| {
            w.set_lp_clock_sync_enable(true);
        });
        rtc.modify_slow_clock_calibration(|w| {
            w.set_value(bits(slow_clock_calibration.value()));
        });
    }
}
