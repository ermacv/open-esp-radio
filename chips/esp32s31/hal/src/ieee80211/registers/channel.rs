//! The MAC side of a PHY channel switch.

use oer_esp32s31_pac::{
    modem_pwr::regdma_control::ActiveLink, wifi_mac::control::PowerSaveTxBlock,
};
use oer_mmio::Bits;

use super::{MacPlacement, WifiRadioRegisters, byte};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Request the complete `WIFI_PS_NONE` MAC stop used before retuning PHY.
    ///
    /// SOURCE: `BLOB_LIBPP_MAC_CHANNEL_SWITCH`, specifically complete
    /// `libpp[hal_mac.o]::hal_mac_deinit`, and the exact transcription retained in
    /// `PROMOTED_CHANNEL_SWITCH`. With the all-ones no-power-save retention
    /// mask the vendor leaf sets bit 12 and bits 23:16 in one RMW.
    pub fn request_mac_channel_stop_without_power_save(&mut self) {
        self.mac.wifi_mac_control.modify_control(|w| {
            w.set_no_retention_stop_request(true)
                .set_tx_block_stop_request_low(true)
                .set_power_save_tx_block(PowerSaveTxBlock::Blocked)
                .set_tx_block_stop_request_high(Bits::truncate(
                    oer_esp32s31_pac::wifi_mac::control::TxBlockStopRequestHigh::StopAll.bits(),
                ));
        });
        oer_riscv::fence();
    }

    /// Block or unblock the TX queues for station power management.
    ///
    /// SOURCE: complete pinned `libpp[pm.o]::pm_coex_go_to_sleep` and
    /// `pm_coex_schm_process` set bits 19:17 at `0x2010_4cac`, and
    /// `hal_pm_unblock_txq` clears them, each through one fresh-read RMW.
    pub fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.mac.wifi_mac_control.modify_control(|w| {
            w.set_power_save_tx_block(if blocked {
                PowerSaveTxBlock::Blocked
            } else {
                PowerSaveTxBlock::Run
            });
        });
        oer_riscv::fence();
    }

    /// Sample the three activity bits polled by the vendor channel switch.
    pub fn mac_channel_active_state(&self) -> u8 {
        byte(self.mac.wifi_mac_control.read_control().active_state())
    }

    /// Clear the no-power-save MAC stop request after PHY retuning.
    pub fn resume_mac_channel_without_power_save(&mut self) {
        self.mac.wifi_mac_control.modify_control(|w| {
            w.set_no_retention_stop_request(false)
                .set_tx_block_stop_request_low(false)
                .set_power_save_tx_block(PowerSaveTxBlock::Run)
                .set_tx_block_stop_request_high(Bits::truncate(
                    oer_esp32s31_pac::wifi_mac::control::TxBlockStopRequestHigh::Run.bits(),
                ));
        });
        oer_riscv::fence();
    }

    /// Select the Wi-Fi no-power-save REGDMA link.
    pub fn select_wifi_no_power_save_regdma_link(&mut self) {
        self.mac.wifi_mac_regdma_control.modify_regdma_control(|w| {
            w.set_active_link(ActiveLink::WifiNoPowerSave);
        });
        oer_riscv::fence();
    }

    /// Read the active REGDMA link for diagnostics and hardware assertions.
    pub fn wifi_mac_regdma_link(&self) -> u8 {
        byte(
            self.mac
                .wifi_mac_regdma_control
                .read_regdma_control()
                .active_link(),
        )
    }
}

#[cfg(test)]
mod tests;
