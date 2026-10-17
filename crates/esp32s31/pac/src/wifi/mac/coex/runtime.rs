//! Closed runtime Wi-Fi COEX/PTI register leaves.

#![forbid(unsafe_code)]

use crate::{MacItwtClearIndex, MacPti, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Publish receive-beacon and shared receive PTI in vendor instruction order.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_set_rx_beacon_pti`.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        let runtime = &self.peripherals.wifi_mac.wifi_mac_coex_runtime;
        crate::generated::publish_mac_rx_beacon_pti(
            runtime,
            crate::generated::MacRxBeaconPtiMaskedInput::new(beacon.get() << 12),
        );
        crate::generated::publish_mac_shared_rx_pti(
            runtime,
            crate::generated::MacSharedRxPtiMaskedInput::new(shared.get()),
        );
    }

    /// Set the hardware beacon receive window and time.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_coex.o]::hal_set_rx_beacon_time`
    /// replaces bits 15:0 at `0x2010_d854` with the window, then bits 7:0 at
    /// `0x2010_d8a0` with bits 15:8 of the time.
    pub fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32) {
        self.peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .rx_beacon_time_low()
            .modify(|_, w| w.value().set(window_micros));
        self.peripherals
            .wifi_mac
            .wifi_mac_coex_runtime
            .rx_beacon_time_high()
            .modify(|_, w| w.value().set((time_micros >> 8) as u8));
    }

    /// Set the reviewed receive-beacon clear request without exposing the
    /// remaining unknown request bits.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_clear_rx_beacon_pti`.
    pub fn clear_rx_beacon_pti(&mut self) {
        crate::generated::request_mac_rx_beacon_clear(
            &self.peripherals.wifi_mac.wifi_mac_coex_runtime,
            crate::generated::MacRxBeaconClearRequest::Beacon,
        );
    }

    /// Publish the two complete `hal_set_itwt_pti` register edges.
    ///
    /// The first vendor argument is reduced to whether it is zero. The second
    /// contributes only its low PTI nibble. The control edge replaces its low
    /// byte, while the shared-PTI edge preserves bits 31:4 and replaces only
    /// bits 3:0.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        let runtime = &self.peripherals.wifi_mac.wifi_mac_coex_runtime;
        crate::generated::publish_mac_itwt_control(
            runtime,
            crate::generated::MacItwtControlMaskedInput::new(u32::from(argument_is_zero)),
        );
        crate::generated::publish_mac_shared_rx_pti(
            runtime,
            crate::generated::MacSharedRxPtiMaskedInput::new(shared.get()),
        );
    }

    /// Set one machine-bounded request bit like complete `hal_clr_itwt_pti`.
    ///
    /// The individual bit meanings remain unknown. The public index type only
    /// exposes the five-bit shift domain implemented by the instruction.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        crate::generated::request_mac_itwt_clear(
            &self.peripherals.wifi_mac.wifi_mac_coex_runtime,
            index,
        );
    }
}
