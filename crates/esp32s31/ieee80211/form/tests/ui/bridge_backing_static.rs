#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::commands::{bridge::{Bridged, ExecutionStorage}, CommandAuthority};
use oer_ieee80211_lower_mac::{Ieee80211Capabilities, LowerMacCapabilities};
use oer_radio_port::Root;
pub struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Interface;
    const CAPS: LowerMacCapabilities = oer_esp32s31_ieee80211::lower_mac::esp32s31_lower_mac_capabilities(2048);
}

#[cfg(ui_reject)]
type Backing<'a> = &'a mut ExecutionStorage<4,9>;
#[cfg(not(ui_reject))]
type Backing<'a> = &'static mut ExecutionStorage<4,9>;
pub fn bind<'id>(radio:RadioRuntimeOwner, authority:CommandAuthority<'id,Root,0,0>, storage:Backing<'_>) {
    let _ = Bridged::<RadioRuntimeOwner,4,9>::bind::<Caps,(),(),(),(),0,0>(radio,authority,storage); // ui-error: E0521
}
