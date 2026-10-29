#![no_std]
use oer_esp32s31_ieee80211_driver::mac::irq::consumer::IrqStorage;
pub fn inspect<E: oer_espressif_interrupt_table::signal::SignalExecutor>(storage: &mut IrqStorage<E>) {
    let (publisher, mut consumer) = storage.split();
    #[cfg(ui_reject)]
    let _count = publisher.diagnostics().rx_post_count(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    {
        publisher.notify_rx_capacity();
        let _batch = consumer.take_batch();
    }
}
