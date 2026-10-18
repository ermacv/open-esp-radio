#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{Configuration, SetupSender};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
pub fn enqueue<'id, Caps: Ieee80211Capabilities>(
    sender: &mut SetupSender<'_, 'id, Caps, (), (), (), (), 0, 0, 4, 9>,
    configuration: Configuration<'id, Caps, (), ()>,
) {
    #[cfg(ui_reject)]
    sender.configure(configuration);
    sender.configure(configuration); // ui-error: E0382
}
