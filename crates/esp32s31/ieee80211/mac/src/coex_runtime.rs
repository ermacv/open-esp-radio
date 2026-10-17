//! Typed LMAC boundary for complete runtime Wi-Fi COEX register leaves.

use oer_esp32s31_hal::ieee80211::mac::WifiMacHal;
use oer_esp32s31_hal::ieee80211::mac::{
    MacItwtClearIndex, MacPti, MacTxPtiProgram, MacTxQueueIndex,
};

/// Minimal hardware authority required by receive-beacon coexistence policy.
pub trait MacRuntimeCoexHardware {
    fn publish_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti);
    fn clear_rx_beacon_pti_request(&mut self);
    fn publish_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti);
    fn clear_itwt_pti_request(&mut self, index: MacItwtClearIndex);
    fn publish_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram);
}

impl MacRuntimeCoexHardware for WifiMacHal<'_> {
    fn publish_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        self.set_rx_beacon_pti(beacon, shared);
    }

    fn clear_rx_beacon_pti_request(&mut self) {
        self.clear_rx_beacon_pti();
    }

    fn publish_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        self.set_itwt_pti(argument_is_zero, shared);
    }

    fn clear_itwt_pti_request(&mut self, index: MacItwtClearIndex) {
        self.clear_itwt_pti(index);
    }

    fn publish_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        self.set_tx_pti(queue, program);
    }
}

/// Publish the exact `hal_set_rx_beacon_pti` register transaction.
pub fn configure_rx_beacon_pti<H: MacRuntimeCoexHardware>(
    hardware: &mut H,
    beacon: MacPti,
    shared: MacPti,
) {
    hardware.publish_rx_beacon_pti(beacon, shared);
}

/// Publish the exact `hal_clear_rx_beacon_pti` register transaction.
pub fn clear_rx_beacon_pti<H: MacRuntimeCoexHardware>(hardware: &mut H) {
    hardware.clear_rx_beacon_pti_request();
}

/// Publish the exact `hal_set_itwt_pti` register transaction.
pub fn configure_itwt_pti<H: MacRuntimeCoexHardware>(
    hardware: &mut H,
    argument_is_zero: bool,
    shared: MacPti,
) {
    hardware.publish_itwt_pti(argument_is_zero, shared);
}

/// Publish the exact `hal_clr_itwt_pti` register transaction.
pub fn clear_itwt_pti<H: MacRuntimeCoexHardware>(hardware: &mut H, index: MacItwtClearIndex) {
    hardware.clear_itwt_pti_request(index);
}

/// Publish the exact `hal_set_tx_pti` register transaction.
pub fn configure_tx_pti<H: MacRuntimeCoexHardware>(
    hardware: &mut H,
    queue: MacTxQueueIndex,
    program: MacTxPtiProgram,
) {
    hardware.publish_tx_pti(queue, program);
}

#[cfg(test)]
mod tests;
