//! Generated-PAC ownership for the MAC side of a PHY channel switch.

#![forbid(unsafe_code)]

use crate::{WifiRadioRegisters, device_fence};

impl WifiRadioRegisters {
    /// Request the complete `WIFI_PS_NONE` MAC stop used before retuning PHY.
    ///
    /// SOURCE: `BLOB_LIBPP_MAC_CHANNEL_SWITCH`, specifically complete
    /// `libpp[hal_mac.o]::hal_mac_deinit`, and the exact transcription retained in
    /// `PROMOTED_CHANNEL_SWITCH`. With the all-ones no-power-save retention
    /// mask the vendor leaf sets bit 12 and bits 23:16 in one RMW.
    pub fn request_mac_channel_stop_without_power_save(&mut self) {
        let control = self.peripherals.wifi_mac.wifi_mac_control.control();
        control.modify(|_, w| {
            w.no_retention_stop_request()
                .set_bit()
                .tx_block_stop_request_low()
                .set_bit()
                .power_save_tx_block()
                .blocked()
                .tx_block_stop_request_high()
                .set(0xF)
        });
        device_fence();
    }

    /// Block or unblock the TX queues for station power management.
    ///
    /// SOURCE: complete pinned `libpp[pm.o]::pm_coex_go_to_sleep` and
    /// `pm_coex_schm_process` set bits 19:17 at `0x2010_4cac`, and
    /// `hal_pm_unblock_txq` clears them, each through one fresh-read RMW.
    pub fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.peripherals
            .wifi_mac
            .wifi_mac_control
            .control()
            .modify(|_, w| {
                if blocked {
                    w.power_save_tx_block().blocked()
                } else {
                    w.power_save_tx_block().run()
                }
            });
        device_fence();
    }

    /// Sample the three activity bits polled by the vendor channel switch.
    pub fn mac_channel_active_state(&self) -> u8 {
        self.peripherals
            .wifi_mac
            .wifi_mac_control
            .control()
            .read()
            .active_state()
            .bits()
    }

    /// Clear the no-power-save MAC stop request after PHY retuning.
    pub fn resume_mac_channel_without_power_save(&mut self) {
        let control = self.peripherals.wifi_mac.wifi_mac_control.control();
        control.modify(|_, w| {
            w.no_retention_stop_request()
                .clear_bit()
                .tx_block_stop_request_low()
                .clear_bit()
                .power_save_tx_block()
                .run()
                .tx_block_stop_request_high()
                .set(0)
        });
        device_fence();
    }

    /// Select the Wi-Fi no-power-save REGDMA link.
    pub fn select_wifi_no_power_save_regdma_link(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_regdma_control
            .control()
            .modify(|_, w| w.active_link().wifi_no_power_save());
        device_fence();
    }

    /// Read the active REGDMA link for diagnostics and hardware assertions.
    pub fn wifi_mac_regdma_link(&self) -> u8 {
        self.peripherals
            .wifi_mac
            .wifi_mac_regdma_control
            .control()
            .read()
            .active_link()
            .bits()
    }
}
