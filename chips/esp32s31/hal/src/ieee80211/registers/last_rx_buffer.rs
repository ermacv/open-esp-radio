//! The complete last-RX-buffer table initialization.

use super::{MacPlacement, WifiRadioRegisters, bits, element};

/// One last-RX-buffer table entry's three words, in the vendor's store order.
struct LastRxBufferEntry {
    control: u32,
    parameter_a: u32,
    parameter_b: u32,
}

/// The six entries `mac_last_rxbuf_init` stores, in order.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size
/// `0xd2` (BLOB_LIBPP_MAC_LAST_RXBUF_INIT).
const LAST_RX_BUFFER_ENTRIES: [LastRxBufferEntry; 6] = [
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_0608,
        parameter_b: 0x0000_ffff,
    },
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_0808,
        parameter_b: 0x0000_ffff,
    },
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_8e88,
        parameter_b: 0x0000_ffff,
    },
    LastRxBufferEntry {
        control: 0x0002_301c,
        parameter_a: 0x4400_4300,
        parameter_b: 0xffff_ffff,
    },
    LastRxBufferEntry {
        control: 0x0002_301c,
        parameter_a: 0x4300_4400,
        parameter_b: 0xffff_ffff,
    },
    LastRxBufferEntry {
        control: 0x0002_3011,
        parameter_a: 0x0000_0001,
        parameter_b: 0x0000_00ff,
    },
];

/// The bits `ENTRY_PARAMETER_B%s`'s one described field holds.
const PARAMETER_B_FIELD: u32 = 0x0000_0fff;

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Publish all six table entries, then their three enable edges.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size `0xd2`.
    pub fn initialize_mac_last_rx_buffer(&mut self) {
        let table = &mut self.mac.wifi_mac_last_rx_buffer;
        for (position, entry) in LAST_RX_BUFFER_ENTRIES.iter().enumerate() {
            let index = element::<6>(position);
            table.write_entry_control(index, |w| {
                w.set_bits_0_31_opaque(entry.control);
            });
            table.write_entry_parameter_a(index, |w| {
                w.set_bits_0_31_opaque(entry.parameter_a);
            });
            if entry.parameter_b & !PARAMETER_B_FIELD != 0 {
                todo!(
                    "PAC gap: WIFI_MAC.ENTRY_PARAMETER_B%s whole-word image above its 12-bit field"
                );
            }
            table.write_entry_parameter_b(index, |w| {
                w.set_bits_0_11_opaque(bits(entry.parameter_b));
            });
        }

        // Preserve the two distinct fresh-read edges from the complete leaf.
        table.modify_last_rx_buffer_control(|w| {
            w.set_bits_8_13_opaque(bits(0x3f));
        });
        table.modify_last_rx_buffer_control(|w| {
            w.set_bits_1_6_opaque(bits(0x3f));
        });
        self.mac.wifi_mac_rx_csi_control.modify_rx_csi_control(|w| {
            w.set_bit_27_opaque(true);
        });
    }
}

#[cfg(test)]
mod tests;
