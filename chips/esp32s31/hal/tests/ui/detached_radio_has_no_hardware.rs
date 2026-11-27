#![no_std]
use oer_chips_esp32s31_hal::{radio::domain::transaction::DetachedRadioDomain, power::PlatformClockBackend};
pub fn observe<B: PlatformClockBackend>(mut owner: DetachedRadioDomain<B>) {
    #[cfg(ui_reject)]
    let _hardware = owner.hardware_mut(); // ui-error: E0599
}
