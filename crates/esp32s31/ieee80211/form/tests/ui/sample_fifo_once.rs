#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{Sampling, PhaseSender};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
pub fn enqueue<'id, Caps: Ieee80211Capabilities>(
    sender: &mut PhaseSender<'_, 'id, Caps, (), (), (), (), 0, 0, 4, 9>,
    sampling: Sampling<'id>,
) {
    #[cfg(ui_reject)]
    sender.sample_tsf(sampling);
    sender.sample_tsf(sampling); // ui-error: E0382
}
