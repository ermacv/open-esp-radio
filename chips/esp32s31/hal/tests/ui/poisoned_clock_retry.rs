#![no_std]
use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee80211::client::{ClockPoisoned, StoppedInstallation},
};
pub fn retain<B: oer_esp32s31_hal::power::PlatformClockBackend, T>(
    stopped: StoppedInstallation,
    obligation: ClockPoisoned,
    _lease: &mut RadioDomain<B, T>,
) -> (StoppedInstallation, ClockPoisoned) {
    #[cfg(ui_reject)]
    obligation.release_clocks(_lease); // ui-error: E0599
    (stopped, obligation)
}
