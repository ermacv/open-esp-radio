//! The complete last-RX-buffer table initialization.

use oer_esp32s31_pac::wifi_mac::WifiMacLastRxBuffer;
use oer_mmio::Placement;

use super::{MacPlacement, WifiRadioRegisters, bits, element};

/// One last-RX-buffer table entry's three words, in the vendor's store order.
struct LastRxBufferEntry {
    control: u32,
    parameter_a: u32,
    parameter_b: EntryParameterB,
}

/// Which store `ENTRY_PARAMETER_B%s` of one entry takes.
#[derive(Clone, Copy)]
enum EntryParameterB {
    /// The vendor's whole-word image (entries zero to four).
    WholeWord,
    /// The described twelve-bit field, stored from zero.
    Field(u32),
}

/// The six entries `mac_last_rxbuf_init` stores, in order.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size
/// `0xd2` (BLOB_LIBPP_MAC_LAST_RXBUF_INIT). The whole-word images of
/// entries zero to four (`0x0000ffff` three times, then `0xffffffff` twice)
/// are the PAC's sourced sequence steps.
const LAST_RX_BUFFER_ENTRIES: [LastRxBufferEntry; 6] = [
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_0608,
        parameter_b: EntryParameterB::WholeWord,
    },
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_0808,
        parameter_b: EntryParameterB::WholeWord,
    },
    LastRxBufferEntry {
        control: 0x0002_3006,
        parameter_a: 0x0000_8e88,
        parameter_b: EntryParameterB::WholeWord,
    },
    LastRxBufferEntry {
        control: 0x0002_301c,
        parameter_a: 0x4400_4300,
        parameter_b: EntryParameterB::WholeWord,
    },
    LastRxBufferEntry {
        control: 0x0002_301c,
        parameter_a: 0x4300_4400,
        parameter_b: EntryParameterB::WholeWord,
    },
    LastRxBufferEntry {
        control: 0x0002_3011,
        parameter_a: 0x0000_0001,
        parameter_b: EntryParameterB::Field(0x0000_00ff),
    },
];

/// The PAC's whole-word `ENTRY_PARAMETER_B` step of entry `position`.
fn whole_word_step<P: Placement>(table: &mut WifiMacLastRxBuffer<P>, position: usize) {
    match position {
        0 => table.initialize_mac_last_rx_buffer_entries_entry_parameter_b0(),
        1 => table.initialize_mac_last_rx_buffer_entries_entry_parameter_b1(),
        2 => table.initialize_mac_last_rx_buffer_entries_entry_parameter_b2(),
        3 => table.initialize_mac_last_rx_buffer_entries_entry_parameter_b3(),
        4 => table.initialize_mac_last_rx_buffer_entries_entry_parameter_b4(),
        _ => unreachable!("only entries zero to four store a whole-word image"),
    }
}

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
            match entry.parameter_b {
                EntryParameterB::WholeWord => whole_word_step(table, position),
                EntryParameterB::Field(value) => table.write_entry_parameter_b(index, |w| {
                    w.set_bits_0_11_opaque(bits(value));
                }),
            }
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
