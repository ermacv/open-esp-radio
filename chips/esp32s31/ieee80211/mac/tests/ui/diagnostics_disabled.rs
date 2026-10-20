#![no_std]
use oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage;
pub fn inspect(storage: &mut IrqStorage) {
    let (publisher, mut consumer) = storage.split();
    #[cfg(ui_reject)]
    let _count = publisher.diagnostics().rx_post_count(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    {
        publisher.notify_rx_capacity();
        let _batch = consumer.take_batch();
    }
}
