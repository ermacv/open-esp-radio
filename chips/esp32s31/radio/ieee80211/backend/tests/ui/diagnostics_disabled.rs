#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::mac::irq::consumer::IrqStorage;
pub fn inspect<E: oer_chips_family_platform_interrupt_table::signal::SignalExecutor>(storage: &mut IrqStorage<E>) {
    let (publisher, mut consumer) = storage.split();
    #[cfg(ui_reject)]
    let _count = publisher.diagnostics().rx_post_count(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    {
        publisher.notify_rx_capacity();
        let _batch = consumer.take_batch();
    }
}
