#![no_std]
use oer_chips_esp32s31_hal::{radio::domain::{RadioDomain, transaction::Operation}, radio::ieee80211::client::WifiClocked, power::PlatformClockBackend};
pub fn finish<B: PlatformClockBackend>(wifi: &mut WifiClocked, radio: RadioDomain<B>) -> RadioDomain<B> {
    let mut platform = ();
    let channel = wifi.channel_transaction(&mut platform, radio.detach(Operation::Channel));
    let radio = channel.reunite();
    #[cfg(ui_reject)]
    let _again = channel.reunite(); // ui-error: E0382
    radio
}
