//! Generated-PAC ownership for the open promiscuous receive boundary.

#![forbid(unsafe_code)]

use crate::{WifiRadioRegisters, device_fence};

impl WifiRadioRegisters {
    /// Configure the complete working open promiscuous receive frontier.
    ///
    /// SOURCE: `OPEN_DRIVER_PROMISCUOUS_RX_FRONTIER`, complete
    /// `BLOB_LIBPP_HAL_SNIFFER_ENABLE`, and the register identity independently
    /// confirmed by `BLOB_LIBPP_HAL_SNIFFER_MISC`.
    pub fn configure_open_mac_promiscuous_receive(&mut self) {
        // Keep MAC_CONTROL intact: its stop request belongs to the runtime
        // lifecycle and is released only with a prepared, live walker.
        let filter = &self.peripherals.wifi_mac.wifi_mac_rx_filter;
        let control = filter.policy(3);
        // Every line below is a separate fresh-read RMW in the complete
        // hal_sniffer_enable leaf.
        control.modify(|_, w| w.auto_ack_disable().set_bit());
        control.modify(|_, w| w.dump_unicast_check_da().clear_bit());
        control.modify(|_, w| w.dump_unicast_check_bssid().clear_bit());
        control.modify(|_, w| w.dump_broadcast_check_bssid().clear_bit());
        control.modify(|_, w| w.abort_group().clear_bit());
        control.modify(|_, w| w.receive_unicast_check_bssid().clear_bit());
        control.modify(|_, w| {
            w.receive_broadcast_check_bssid()
                .clear_bit()
                .receive_unicast_check_da()
                .clear_bit()
        });

        // The open frontier enables all eight hardware-qualified miscellaneous
        // packet classes while preserving the other mode-dependent fields.
        filter
            .misc_packet_policy()
            .modify(|_, w| w.open_misc_packet_classes().set(0xff));
        device_fence();
    }
}
