//! Cold MAC preparation keeps the core gated until RX DMA is ready.

use crate::{ieee80211::mac::WifiMacColdHal, types::MacInterruptMask};

pub trait MacColdPrepareHardware {
    fn hold_mac_runtime(&mut self);
    fn prepare_mac_interrupts(&mut self, event_mask: MacInterruptMask);
}

impl MacColdPrepareHardware for WifiMacColdHal<'_> {
    fn hold_mac_runtime(&mut self) {
        WifiMacColdHal::hold_mac_runtime(self);
    }

    fn prepare_mac_interrupts(&mut self, event_mask: MacInterruptMask) {
        self.prepare_interrupts(event_mask);
    }
}
