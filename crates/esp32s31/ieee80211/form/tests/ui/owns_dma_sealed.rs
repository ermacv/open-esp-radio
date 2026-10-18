#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::OwnsDma;

// OwnsDma is sealed: only the form states which DMA owner a driver's hardware
// has and where its key copies live, so no outside type can claim to own no
// executor while it holds one.
fn installable<H: OwnsDma>() {}

#[cfg(not(ui_reject))]
pub fn bare_radio() {
    installable::<RadioRuntimeOwner>();
}

#[cfg(ui_reject)]
pub struct Radio;
#[cfg(ui_reject)]
impl OwnsDma for Radio {} // ui-error: E0277
#[cfg(ui_reject)]
pub fn bare_radio() {
    installable::<Radio>();
}
