//! The complete cold MAC antenna transaction.

use oer_mmio::Index;

use super::{MacPlacement, WifiRadioRegisters, every_index};

/// The four physical banks of `hal_attenna_init`, in its descending
/// traversal order.
const DESCENDING_BANKS: [Index<4>; 4] = {
    let [b0, b1, b2, b3] = every_index::<4>();
    [b3, b2, b1, b0]
};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply all 34 fresh-read RMW edges of `hal_attenna_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_tx.o]`
    /// `hal_attenna_init`, size `0x5e`. The vendor symbol's original spelling
    /// is intentional. Both array traversals run from `0x20105510` down to
    /// `0x201051ac` with a `0x7c` stride.
    pub fn initialize_mac_antenna(&mut self) {
        let init = &mut self.mac.wifi_mac_antenna_init;
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;

        // First complete reverse traversal: one fresh-read edge per word.
        for bank in DESCENDING_BANKS {
            vectors.modify_length_control(bank, |w| {
                w.set_txrx_queue_antenna_enable(false);
            });
        }
        for bank in DESCENDING_BANKS {
            init.modify_bank_control(bank, |w| {
                w.set_bit_2_opaque(false);
            });
        }

        // Second complete reverse traversal: the blob deliberately samples
        // each word again before every one of these three updates.
        for bank in DESCENDING_BANKS {
            vectors.modify_length_control(bank, |w| {
                w.set_tx_queue_antenna_last(false);
            });
            vectors.modify_length_control(bank, |w| {
                w.set_tx_queue_antenna_enable(true);
            });
            vectors.modify_length_control(bank, |w| {
                w.set_tx_queue_antenna_value(false);
            });
        }
        for bank in DESCENDING_BANKS {
            init.modify_bank_control(bank, |w| {
                w.set_bit_3_opaque(false);
            });
            init.modify_bank_control(bank, |w| {
                w.set_bit_5_opaque(true);
            });
            init.modify_bank_control(bank, |w| {
                w.set_bit_4_opaque(false);
            });
        }

        init.modify_common_control(|w| {
            w.set_bit_2_opaque(false);
        });
        init.modify_common_control(|w| {
            w.set_bit_5_opaque(true);
        });
    }
}
