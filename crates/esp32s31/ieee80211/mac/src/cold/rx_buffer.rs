//! Ownership boundary for RX buffer geometry before ring publication.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

pub trait MacColdRxBufferHardware {
    fn initialize_rx_buffer_prefix(&mut self);
}

impl MacColdRxBufferHardware for WifiMacColdHal<'_> {
    fn initialize_rx_buffer_prefix(&mut self) {
        WifiMacColdHal::initialize_rx_buffer_prefix(self);
    }
}
