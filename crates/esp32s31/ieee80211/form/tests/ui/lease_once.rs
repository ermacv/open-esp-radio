#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::ScopeLease;

pub fn transfer(lease: ScopeLease<RadioRuntimeOwner>) -> ScopeLease<RadioRuntimeOwner> {
    #[cfg(ui_reject)]
    let _first = lease;
    lease // ui-error: E0382
}
