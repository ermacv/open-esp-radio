//! Ownership boundary for the complete cold MAC enable edge.

use oer_esp32s31_hal::{ieee80211::mac::WifiMacColdHal, types::MacInterruptMask};

pub trait MacColdEnableHardware {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask);
}

impl MacColdEnableHardware for WifiMacColdHal<'_> {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask) {
        self.enable_interrupts(event_mask);
    }
}
