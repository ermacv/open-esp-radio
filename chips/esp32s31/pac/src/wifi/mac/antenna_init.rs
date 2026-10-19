//! Generated-PAC ownership for the complete cold MAC antenna transaction.

use crate::WifiRadioRegisters;

impl WifiRadioRegisters {
    /// Apply all 34 fresh-read RMW edges of `hal_attenna_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_tx.o]`
    /// `hal_attenna_init`, size `0x5e`. The vendor symbol's original spelling
    /// is intentional. Both array traversals run from `0x20105510` down to
    /// `0x201051ac` with a `0x7c` stride.
    pub fn initialize_mac_antenna(&mut self) {
        let init = &self.peripherals.wifi_mac.wifi_mac_antenna_init;
        let vectors = &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector;

        // First complete reverse traversal: one fresh-read edge per word.
        for physical_bank in (4..8).rev() {
            vectors
                .length_control(physical_bank - 4)
                .modify(|_, w| w.txrx_queue_antenna_enable().clear_bit());
        }
        for physical_bank in (0..4).rev() {
            init.bank_control(physical_bank)
                .modify(|_, w| w.bit_2_opaque().clear_bit());
        }

        // Second complete reverse traversal: the blob deliberately samples
        // each word again before every one of these three updates.
        for physical_bank in (4..8).rev() {
            let control = vectors.length_control(physical_bank - 4);
            control.modify(|_, w| w.tx_queue_antenna_last().clear_bit());
            control.modify(|_, w| w.tx_queue_antenna_enable().set_bit());
            control.modify(|_, w| w.tx_queue_antenna_value().clear_bit());
        }
        for physical_bank in (0..4).rev() {
            let control = init.bank_control(physical_bank);
            control.modify(|_, w| w.bit_3_opaque().clear_bit());
            control.modify(|_, w| w.bit_5_opaque().set_bit());
            control.modify(|_, w| w.bit_4_opaque().clear_bit());
        }

        init.common_control()
            .modify(|_, w| w.bit_2_opaque().clear_bit());
        init.common_control()
            .modify(|_, w| w.bit_5_opaque().set_bit());
    }
}
