//! The open promiscuous receive boundary.

use super::{MacPlacement, WifiRadioRegisters};
use oer_mmio::Index;

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Configure the complete working open promiscuous receive frontier.
    ///
    /// SOURCE: `OPEN_DRIVER_PROMISCUOUS_RX_FRONTIER`, complete
    /// `BLOB_LIBPP_HAL_SNIFFER_ENABLE`, and the register identity independently
    /// confirmed by `BLOB_LIBPP_HAL_SNIFFER_MISC`.
    pub fn configure_open_mac_promiscuous_receive(&mut self) {
        // Keep MAC_CONTROL intact: its stop request belongs to the runtime
        // lifecycle and is released only with a prepared, live walker.
        let filter = &mut self.mac.wifi_mac_rx_filter;
        let policy = Index::<4>::constant::<3>();
        // Every line below is a separate fresh-read RMW in the complete
        // hal_sniffer_enable leaf.
        filter.modify_policy(policy, |w| {
            w.set_auto_ack_disable(true);
        });
        filter.modify_policy(policy, |w| {
            w.set_dump_unicast_check_da(false);
        });
        filter.modify_policy(policy, |w| {
            w.set_dump_unicast_check_bssid(false);
        });
        filter.modify_policy(policy, |w| {
            w.set_dump_broadcast_check_bssid(false);
        });
        filter.modify_policy(policy, |w| {
            w.set_abort_group(false);
        });
        filter.modify_policy(policy, |w| {
            w.set_receive_unicast_check_bssid(false);
        });
        filter.modify_policy(policy, |w| {
            w.set_receive_broadcast_check_bssid(false)
                .set_receive_unicast_check_da(false);
        });

        // The open frontier enables all eight hardware-qualified miscellaneous
        // packet classes while preserving the other mode-dependent fields.
        filter.modify_misc_packet_policy(|w| {
            w.set_open_misc_packet_classes(0xff);
        });
        oer_riscv::fence();
    }
}

#[cfg(test)]
mod tests;
