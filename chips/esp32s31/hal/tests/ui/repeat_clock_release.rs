#![no_std]
use oer_esp32s31_hal::{ieee80211::client::WifiClocked, domain::RadioDomain};
pub fn release<T>(clocked: WifiClocked, radio: &mut RadioDomain<T>) {
    let first = clocked.release_clocks(radio);
    #[cfg(ui_reject)]
    let _second = clocked.release_clocks(radio); // ui-error: E0382
    drop(first);
}
