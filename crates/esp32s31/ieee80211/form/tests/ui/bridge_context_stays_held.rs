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

use core::cell::Cell;
fn executor<T: Send + 'static>(_:T) {}
pub fn bind<'id,'context>(radio:RadioRuntimeOwner, authority:CommandAuthority<'id,Root,0,0>, storage:&'static mut ExecutionStorage<4,9>, _: &'context Cell<u32>) {
    let (held, step) = Bridged::<RadioRuntimeOwner,4,9>::bind::<Caps,(),(),&'context Cell<u32>,(),0,0>(radio,authority,storage);
    #[cfg(ui_reject)]
    executor(held); // ui-error: E0277
    #[cfg(not(ui_reject))]
    { executor(step); core::hint::black_box(held); }
}
