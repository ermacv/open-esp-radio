#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{SetupSender, RetuneKind};
use oer_ieee80211_lower_mac::{Ieee80211Capabilities, Channel};
use oer_radio_port::{Slot, Child, Root};
pub fn enqueue<'id, Caps: Ieee80211Capabilities>(
    sender: SetupSender<'_, 'id, Caps, (), (), (), (), 0, 0, 4, 9>,
    permission: Slot<'id, RetuneKind, Child<Root, 7>>, channel: Channel,
) {
    #[cfg(ui_reject)]
    let mut sender = sender.enable();
    #[cfg(not(ui_reject))]
    let mut sender = sender;
    sender.retune(permission, channel); // ui-error: E0599
}
