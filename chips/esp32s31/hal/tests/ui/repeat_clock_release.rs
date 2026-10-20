#![no_std]
use oer_esp32s31_hal::{domain::RadioDomain, ieee80211::client::WifiClocked};
pub fn release<B: oer_esp32s31_hal::power::PlatformClockBackend, T>(
    clocked: WifiClocked,
    radio: &mut RadioDomain<B, T>,
) {
    let first = clocked.release_clocks(radio);
    #[cfg(ui_reject)]
    let _second = clocked.release_clocks(radio); // ui-error: E0382
    drop(first);
}
