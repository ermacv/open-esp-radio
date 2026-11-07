//! The complete cold MAC antenna transaction.

use super::{MacPlacement, WifiRadioRegisters, element};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply all 34 fresh-read RMW edges of `hal_attenna_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_tx.o]`
    /// `hal_attenna_init`, size `0x5e`. The vendor symbol's original spelling
    /// is intentional. Both array traversals run from `0x20105510` down to
    /// `0x201051ac` with a `0x7c` stride: the queue vectors' four
    /// `LENGTH_CONTROL` words, then the antenna part's four `BANK_CONTROL`
    /// words, each from its last element down.
    pub fn initialize_mac_antenna(&mut self) {
        // First complete reverse traversal: one fresh-read edge per word.
        for bank in (0..4).rev() {
            self.mac
                .wifi_mac_tx_queue_vector
                .modify_length_control(element(bank), |w| {
                    w.set_txrx_queue_antenna_enable(false);
                });
        }
        for bank in (0..4).rev() {
            self.mac
                .wifi_mac_antenna_init
                .modify_bank_control(element(bank), |w| {
                    w.set_bit_2_opaque(false);
                });
        }

        // Second complete reverse traversal: the blob deliberately samples
        // each word again before every one of these three updates.
        for bank in (0..4).rev() {
            let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
            vectors.modify_length_control(element(bank), |w| {
                w.set_tx_queue_antenna_last(false);
            });
            vectors.modify_length_control(element(bank), |w| {
                w.set_tx_queue_antenna_enable(true);
            });
            vectors.modify_length_control(element(bank), |w| {
                w.set_tx_queue_antenna_value(false);
            });
        }
        for bank in (0..4).rev() {
            let init = &mut self.mac.wifi_mac_antenna_init;
            init.modify_bank_control(element(bank), |w| {
                w.set_bit_3_opaque(false);
            });
            init.modify_bank_control(element(bank), |w| {
                w.set_bit_5_opaque(true);
            });
            init.modify_bank_control(element(bank), |w| {
                w.set_bit_4_opaque(false);
            });
        }

        let init = &mut self.mac.wifi_mac_antenna_init;
        init.modify_common_control(|w| {
            w.set_bit_2_opaque(false);
        });
        init.modify_common_control(|w| {
            w.set_bit_5_opaque(true);
        });
    }
}

#[cfg(test)]
mod tests;
