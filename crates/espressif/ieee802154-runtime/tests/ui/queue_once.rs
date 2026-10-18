#![no_std]
use oer_espressif_ieee802154_runtime::RuntimeQueue;

pub fn twice(queue: RuntimeQueue, fresh: RuntimeQueue) -> (RuntimeQueue, RuntimeQueue) {
    let first = queue;
    #[cfg(not(ui_reject))]
    let queue = fresh;
    #[cfg(ui_reject)]
    let _ = fresh;
    (first, queue) // ui-error: E0382
}
