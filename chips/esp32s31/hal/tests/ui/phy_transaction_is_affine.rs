#![no_std]
use oer_esp32s31_hal::{domain::RadioDomain, power::PlatformClockBackend};
pub fn complete<B: PlatformClockBackend, T>(radio: RadioDomain<B, T>) -> RadioDomain<B, T> {
    let transaction = radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
    let radio = transaction.reunite();
    #[cfg(ui_reject)]
    let _again = transaction.reunite(); // ui-error: E0382
    radio
}
