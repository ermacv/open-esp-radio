#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::driver::Driver;
pub fn transfer(driver: Driver<'_, RadioRuntimeOwner>) -> Driver<'_, RadioRuntimeOwner> {
    #[cfg(ui_reject)]
    let _first = driver;
    driver // ui-error: E0382
}
