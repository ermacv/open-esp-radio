#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::{ScopeLease, ScopeStorage};

pub fn pin(backing: &'static mut ScopeStorage<RadioRuntimeOwner>) -> ScopeLease<RadioRuntimeOwner> {
    #[cfg(ui_reject)]
    {
        let mut local = ScopeStorage::new();
        ScopeStorage::pin_static(&mut local) // ui-error: E0597
    }
    #[cfg(not(ui_reject))]
    ScopeStorage::pin_static(backing)
}
