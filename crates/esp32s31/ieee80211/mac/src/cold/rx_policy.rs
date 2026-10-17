//! Ownership boundary for the complete cold receive-policy transaction.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

pub trait MacColdRxPolicyHardware {
    fn initialize_cold_receive_policy(&mut self);
}

impl MacColdRxPolicyHardware for WifiMacColdHal<'_> {
    fn initialize_cold_receive_policy(&mut self) {
        self.initialize_receive_policy();
    }
}
