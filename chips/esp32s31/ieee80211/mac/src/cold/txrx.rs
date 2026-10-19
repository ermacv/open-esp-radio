//! Ownership boundary for the complete cold `mac_txrx_init` transaction.

use oer_esp32s31_hal::{ieee80211::mac::WifiMacColdHal, types::MacDelaySlot};

pub trait MacColdTxRxHardware {
    fn initialize_txrx_prefix(&mut self);
    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot);
    fn initialize_txrx_suffix(&mut self);
}

impl MacColdTxRxHardware for WifiMacColdHal<'_> {
    fn initialize_txrx_prefix(&mut self) {
        WifiMacColdHal::initialize_txrx_prefix(self);
    }

    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        WifiMacColdHal::initialize_txrx_callbacks(self, delay_slot);
    }

    fn initialize_txrx_suffix(&mut self) {
        WifiMacColdHal::initialize_txrx_suffix(self);
    }
}
