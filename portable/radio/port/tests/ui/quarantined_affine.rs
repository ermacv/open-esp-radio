#![no_std]
use oer_radio_port::Quarantined;
pub fn retained<S>(receipt: Quarantined<S>) {
    drop(receipt);
    #[cfg(ui_reject)]
    drop(receipt); // ui-error: E0382
}
