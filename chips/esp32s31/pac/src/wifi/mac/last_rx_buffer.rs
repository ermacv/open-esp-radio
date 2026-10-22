//! Generated-PAC ownership for the complete last-RX-buffer table init.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

impl WifiRadioRegisters {
    /// Publish all six table entries, then their three enable edges.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size `0xd2`.
    pub fn initialize_mac_last_rx_buffer(&mut self) {
        let table = &self.peripherals.wifi_mac.wifi_mac_last_rx_buffer;
        crate::svd::fixed_register_sequence::initialize_mac_last_rx_buffer_entries(table);

        // Preserve the two distinct fresh-read edges from the complete leaf.
        table
            .control()
            .modify(|_, w| w.bits_8_13_opaque().set(0x3f));
        table.control().modify(|_, w| w.bits_1_6_opaque().set(0x3f));
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_csi_control
            .control()
            .modify(|_, w| w.bit_27_opaque().set_bit());
    }
}
