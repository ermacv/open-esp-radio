//! System-reset mechanism, independent of the requesting subsystem's policy.

/// Reset the entire SoC through the pinned ESP-HAL system-reset path.
///
/// This is not a CPU-only reset. HAL performs the ESP32-S31 pre-reset handling
/// and invokes the ROM system-reset primitive. The caller decides when reset
/// is required and must retain any still-live peripheral and memory owners
/// until this diverging call. No driver cleanup or protocol acknowledgement is
/// performed here.
///
/// This software request cannot interrupt a stuck caller. It neither arms a
/// hardware watchdog nor establishes a measured reset-to-RF-off time bound.
#[cold]
pub fn reset_system() -> ! {
    esp_hal::system::software_reset()
}
