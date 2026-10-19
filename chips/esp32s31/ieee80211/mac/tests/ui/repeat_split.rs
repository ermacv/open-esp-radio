#![no_std]
use oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage;
pub fn split(storage: &mut IrqStorage) {
    let first = storage.split();
    #[cfg(not(ui_reject))]
    drop(first);
    let second = storage.split(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(first);
    drop(second);
}
