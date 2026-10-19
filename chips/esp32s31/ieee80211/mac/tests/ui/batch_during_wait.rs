#![no_std]
use oer_esp32s31_ieee80211_mac::irq::consumer::IrqConsumer;
pub fn wait(consumer: &mut IrqConsumer<'_>) {
    let wait = consumer.wait();
    #[cfg(not(ui_reject))]
    drop(wait);
    let batch = consumer.take_batch(); // ui-error: E0499
    #[cfg(ui_reject)]
    drop(wait);
    for _work in batch {}
}
