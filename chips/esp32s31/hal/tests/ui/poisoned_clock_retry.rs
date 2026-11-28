#![no_std]
use oer_chips_esp32s31_hal::{
    radio::domain::RadioDomain,
    radio::ieee80211::client::{ClockPoisoned, StoppedInstallation},
};
pub fn retain(
    stopped: StoppedInstallation,
    obligation: ClockPoisoned,
    _lease: &mut RadioDomain,
) -> (StoppedInstallation, ClockPoisoned) {
    #[cfg(ui_reject)]
    obligation.release_clocks(_lease); // ui-error: E0599
    (stopped, obligation)
}
