#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{CommandAuthority, CommandStorage};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_radio_port::Root;
#[cfg(ui_reject)]
const T: usize = 0;
#[cfg(not(ui_reject))]
const T: usize = 2;
pub fn store<'id, Caps: Ieee80211Capabilities>(
    authority: CommandAuthority<'id, Root, 2, 1>,
) -> CommandStorage<'id, Caps, (), (), (), (), T, 1, 4, 13> {
    CommandStorage::new(authority) // ui-error: E0308
}
