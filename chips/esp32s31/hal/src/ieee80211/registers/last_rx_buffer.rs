//! The complete last-RX-buffer table initialization.

use oer_mmio::Bits;

use super::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish all six table entries, then their three enable edges.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size `0xd2`. The entries are
    /// the vendor's whole-word stores, kept as the PAC policy's sequence
    /// `initialize_mac_last_rx_buffer_entries`.
    pub fn initialize_mac_last_rx_buffer(&mut self) {
        let table = &mut self.mac.wifi_mac_last_rx_buffer;
        table.initialize_mac_last_rx_buffer_entries();

        // Preserve the two distinct fresh-read edges from the complete leaf.
        table.modify_control(|w| {
            w.set_bits_8_13_opaque(Bits::constant::<0x3f>());
        });
        table.modify_control(|w| {
            w.set_bits_1_6_opaque(Bits::constant::<0x3f>());
        });
        self.mac.wifi_mac_rx_csi_control.modify_control(|w| {
            w.set_bit_27_opaque(true);
        });
    }
}
