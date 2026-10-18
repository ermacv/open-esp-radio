#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{CommandAuthority, CommandStorage};
use oer_ieee80211_lower_mac::{Ieee80211Capabilities, LowerMacCapabilities};
use oer_radio_port::Root;
pub struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Interface;
    const CAPS: LowerMacCapabilities = oer_esp32s31_ieee80211::lower_mac::esp32s31_lower_mac_capabilities(2048);
}
#[cfg(ui_reject)]
const P: usize = 8;
#[cfg(not(ui_reject))]
const P: usize = 9;
pub fn store(authority: CommandAuthority<'_, Root, 0, 0>) -> CommandStorage<'_, Caps, (), (), (), (), 0, 0, 4, P> {
    CommandStorage::new(authority) // ui-error: E0080
}
