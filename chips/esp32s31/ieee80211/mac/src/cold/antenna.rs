//! Ownership boundary for the complete cold MAC antenna transaction.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

pub trait MacColdAntennaHardware {
    fn initialize_mac_antenna(&mut self);
}

impl MacColdAntennaHardware for WifiMacColdHal<'_> {
    fn initialize_mac_antenna(&mut self) {
        self.initialize_antenna();
    }
}
