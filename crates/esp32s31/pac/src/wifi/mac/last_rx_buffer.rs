//! Generated-PAC ownership for the complete last-RX-buffer table init.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

impl WifiRadioRegisters {
    /// Publish all six table entries, then their three enable edges.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size `0xd2`.
    pub fn initialize_mac_last_rx_buffer(&mut self) {
        const CONTROL: [u32; 6] = [
            0x0002_3006,
            0x0002_3006,
            0x0002_3006,
            0x0002_301c,
            0x0002_301c,
            0x0002_3011,
        ];
        const PARAMETER_A: [u32; 6] = [
            0x0000_0608,
            0x0000_0808,
            0x0000_8e88,
            0x4400_4300,
            0x4300_4400,
            0x0000_0001,
        ];
        const PARAMETER_B: [u32; 6] = [
            0x0000_ffff,
            0x0000_ffff,
            0x0000_ffff,
            0xffff_ffff,
            0xffff_ffff,
            0x0000_00ff,
        ];

        let table = &self.peripherals.wifi_mac.wifi_mac_last_rx_buffer;
        for entry in 0..6 {
            crate::svd::zero_based_field_write::mac_last_rx_buffer_entry_control(
                table,
                entry,
                CONTROL[entry],
            );
            crate::svd::zero_based_field_write::mac_last_rx_buffer_entry_parameter_a(
                table,
                entry,
                PARAMETER_A[entry],
            );
            crate::svd::zero_based_field_write::mac_last_rx_buffer_entry_parameter_b(
                table,
                entry,
                PARAMETER_B[entry],
            );
        }

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
