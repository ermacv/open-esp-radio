#![no_std]
use oer_esp32s31_hal::{ieee80211::client::WifiClocked, shared_radio::SharedRadioLease};
pub fn release<T>(clocked: WifiClocked, lease: &mut SharedRadioLease<'_, T>) {
    let first = clocked.release_clocks(lease);
    #[cfg(ui_reject)]
    let _second = clocked.release_clocks(lease); // ui-error: E0382
    drop(first);
}
