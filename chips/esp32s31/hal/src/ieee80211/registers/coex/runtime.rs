//! Closed runtime Wi-Fi COEX/PTI register leaves.

use super::super::{MacItwtClearIndex, MacPlacement, MacPti, WifiRadioRegisters, bits};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish receive-beacon and shared receive PTI in vendor instruction order.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_set_rx_beacon_pti`.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        let runtime = &mut self.mac.wifi_mac_coex_runtime;
        runtime.modify_rx_beacon_pti(|w| {
            w.set_pti(bits(beacon.get()));
        });
        runtime.modify_shared_rx_pti(|w| {
            w.set_pti(bits(shared.get()));
        });
    }

    /// Set the hardware beacon receive window and time.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_coex.o]::hal_set_rx_beacon_time`
    /// replaces bits 15:0 at `0x2010_d854` with the window, then bits 7:0 at
    /// `0x2010_d8a0` with bits 15:8 of the time.
    pub fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32) {
        self.mac
            .wifi_mac_rtc_timer_update
            .modify_rx_beacon_time_low(|w| {
                w.set_value(window_micros);
            });
        let [_, time_high, _, _] = time_micros.to_le_bytes();
        self.mac
            .wifi_mac_coex_runtime
            .modify_rx_beacon_time_high(|w| {
                w.set_value(time_high);
            });
    }

    /// Set the reviewed receive-beacon clear request without exposing the
    /// remaining unknown request bits.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_clear_rx_beacon_pti`.
    pub fn clear_rx_beacon_pti(&mut self) {
        self.mac.wifi_mac_coex_runtime.modify_clear_request(|w| {
            let requests = w.bits_0_31_opaque()
                | oer_esp32s31_pac::modem_pwr::clear_request::Bits0_31Opaque::RxBeacon.bits();
            w.set_bits_0_31_opaque(requests);
        });
    }

    /// Publish the two complete `hal_set_itwt_pti` register edges.
    ///
    /// The first vendor argument is reduced to whether it is zero. The second
    /// contributes only its low PTI nibble. The control edge replaces its low
    /// byte, while the shared-PTI edge preserves bits 31:4 and replaces only
    /// bits 3:0.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        let runtime = &mut self.mac.wifi_mac_coex_runtime;
        runtime.modify_itwt_control(|w| {
            w.set_bits_0_7_opaque(u8::from(argument_is_zero));
        });
        runtime.modify_shared_rx_pti(|w| {
            w.set_pti(bits(shared.get()));
        });
    }

    /// Set one machine-bounded request bit like complete `hal_clr_itwt_pti`.
    ///
    /// The individual bit meanings remain unknown. The public index type only
    /// exposes the five-bit shift domain implemented by the instruction.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        self.set_clear_request_bit(index.get());
    }

    /// One fresh-read RMW that sets bit `bit` (below 32) of `CLEAR_REQUEST`.
    fn set_clear_request_bit(&mut self, bit: u32) {
        self.mac.wifi_mac_coex_runtime.modify_clear_request(|w| {
            let requests = w.bits_0_31_opaque() | (1 << bit);
            w.set_bits_0_31_opaque(requests);
        });
    }
}
