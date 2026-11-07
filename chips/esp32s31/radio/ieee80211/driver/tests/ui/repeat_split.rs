#![no_std]
use oer_chips_esp32s31_radio_ieee80211_driver::mac::irq::consumer::IrqStorage;
pub fn split<E: oer_chips_family_interrupt_table::signal::SignalExecutor>(storage: &mut IrqStorage<E>) {
    let first = storage.split();
    #[cfg(not(ui_reject))]
    drop(first);
    let second = storage.split(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(first);
    drop(second);
}
