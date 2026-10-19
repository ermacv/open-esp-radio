//! Ownership boundary for the complete last-RX-buffer table init.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

pub trait MacColdLastRxBufferHardware {
    fn initialize_last_rx_buffer_table(&mut self);
}

impl MacColdLastRxBufferHardware for WifiMacColdHal<'_> {
    fn initialize_last_rx_buffer_table(&mut self) {
        WifiMacColdHal::initialize_last_rx_buffer_table(self);
    }
}
