//! The direct pre-COEX tail of `hal_init`.

use oer_chips_esp32s31_pac::wifi_mac::rx_csi_control::{Bits0_7Opaque, Bits8_15Opaque};

use super::{MacPlacement, WifiRadioRegisters, WifiRxDmaRegisters};

/// The OS interface's slow-clock calibration, whole: the field's vendor
/// cut publishes its low eighteen bits, as `hal_timer_update_by_rtc` does.
/// It carries only a register value, not evidence that a clock was measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRtcCalibration(u32);

impl MacRtcCalibration {
    pub const ZERO: Self = Self(0);

    /// The OS interface's number, which the field cuts.
    pub const fn from_osi_value(value: u32) -> Self {
        Self(value)
    }

    /// The OS interface's number.
    pub const fn value(self) -> u32 {
        self.0
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

        rx_dma.enable_mac_rx_hw_beacon_reload();

        let rtc = &mut self.mac.wifi_mac_rtc_timer_update;
        rtc.modify_rtc_timer_update_control(|w| {
            w.set_lp_clock_sync_enable(true);
        });
        rtc.modify_slow_clock_calibration(|w| {
            w.set_slow_clock_calibration_low_bits(slow_clock_calibration.value());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::MacRtcCalibration;
    use crate::radio::ieee80211::registers::HostWords;

    #[test]
    fn the_calibration_s_low_eighteen_bits_are_published_and_its_word_s_others_kept() {
        for (fill, calibration, value) in [
            (0, 0x0001_2345, 0x1_2345),
            // The OS interface's bits above eighteen are the field's cut.
            (0, 0xfffc_0005, 0x5),
            (u32::MAX, 0x0004_0000, 0),
        ] {
            let mut memory = HostWords::filled(fill);
            let mut owners = memory.owners();
            owners.registers.initialize_mac_hal_tail(
                &mut owners.rx_dma,
                MacRtcCalibration::from_osi_value(calibration),
            );
            let word = owners
                .registers
                .mac
                .wifi_mac_rtc_timer_update
                .read_slow_clock_calibration();
            assert_eq!(word.value().get(), value, "{calibration:#x}");
            assert_eq!(
                word.bits_18_31_opaque().get(),
                fill >> 18,
                "{calibration:#x}"
            );
        }
    }
}
