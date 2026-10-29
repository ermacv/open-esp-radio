#![no_std]
use oer_esp32s31_ieee80211_driver::mac::irq::consumer::IrqConsumer;
pub fn wait<E: oer_espressif_interrupt_table::signal::SignalExecutor>(consumer: &mut IrqConsumer<'_, E>) {
    let wait = consumer.wait();
    #[cfg(not(ui_reject))]
    drop(wait);
    let batch = consumer.take_batch(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(wait);
    for _work in batch {}
}
