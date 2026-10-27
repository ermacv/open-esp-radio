#![no_std]
use oer_esp32s31_hal::{domain::transaction::DetachedRadioDomain, power::PlatformClockBackend};
pub fn observe<B: PlatformClockBackend, T>(mut owner: DetachedRadioDomain<B, T>) {
    let _attachment = owner.attachment_mut();
    #[cfg(ui_reject)]
    let _hardware = owner.hardware_mut(); // ui-error: E0599
}
