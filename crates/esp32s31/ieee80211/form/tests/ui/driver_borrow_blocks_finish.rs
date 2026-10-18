#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::driver::Driver;
use oer_radio_port::Retire;

pub async fn finish_after_wait(driver: Driver<'_, RadioRuntimeOwner>, retire: Retire<'_>) {
    let waiting = driver.drive(&retire);
    #[cfg(ui_reject)]
    let _early = driver.finish(); // ui-error: E0505
    let _proof = waiting.await;
    #[cfg(not(ui_reject))]
    let _state = driver.finish();
}
