#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::mac::irq::consumer::IrqConsumer;
pub fn wait<E: oer_chips_family_platform_interrupt_table::signal::SignalExecutor>(consumer: &mut IrqConsumer<'_, E>) {
    let first = consumer.wait();
    #[cfg(not(ui_reject))]
    drop(first);
    let second = consumer.wait(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(first);
    drop(second);
}
