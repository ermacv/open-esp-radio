#![no_std]
use oer_chips_esp32s31_hal::{radio::domain::RadioDomain, power::PlatformClockBackend};
pub fn complete<B: PlatformClockBackend>(radio: RadioDomain<B>) -> RadioDomain<B> {
    let transaction = radio.detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Maintain);
    let radio = transaction.reunite();
    #[cfg(ui_reject)]
    let _again = transaction.reunite(); // ui-error: E0382
    radio
}
