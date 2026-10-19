#![no_std]
use oer_esp32s31_hal::{
    ieee80211::client::{ClockPoisoned, StoppedInstallation},
    domain::RadioDomain,
};
pub fn retain<T>(
    stopped: StoppedInstallation,
    obligation: ClockPoisoned,
    _lease: &mut RadioDomain<T>,
) -> (StoppedInstallation, ClockPoisoned) {
    #[cfg(ui_reject)]
    obligation.release_clocks(_lease); // ui-error: E0599
    (stopped, obligation)
}
