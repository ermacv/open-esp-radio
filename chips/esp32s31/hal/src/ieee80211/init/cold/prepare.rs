//! Cold MAC preparation keeps the core gated until RX DMA is ready.

use crate::ieee80211::mac::WifiMacColdHal;

pub trait MacColdPrepareHardware {
    fn hold_mac_runtime(&mut self);
}

impl MacColdPrepareHardware for WifiMacColdHal<'_> {
    fn hold_mac_runtime(&mut self) {
        WifiMacColdHal::hold_mac_runtime(self);
    }
}
