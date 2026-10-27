#![no_std]
use oer_esp32s31_ieee80211_mac::irq::consumer::PowerIrqConsumer;
pub fn wait<E: oer_espressif_interrupt_table::signal::SignalExecutor>(consumer: &mut PowerIrqConsumer<'_, E>) {
    let first = consumer.wait();
    #[cfg(not(ui_reject))]
    drop(first);
    let second = consumer.wait(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(first);
    drop(second);
}
