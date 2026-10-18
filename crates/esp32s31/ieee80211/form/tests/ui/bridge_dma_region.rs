#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::bridge::DmaSpan;
#[cfg(ui_reject)]
type Region<'a> = &'a mut [u8];
#[cfg(not(ui_reject))]
type Region<'a> = oer_memory::StableDmaRegion<'a>;
pub fn describe(region:Region<'_>)->DmaSpan {
    DmaSpan::from_region(region) // ui-error: E0308
}
