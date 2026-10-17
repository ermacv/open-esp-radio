#![no_std]
use oer_esp32s31_hal::{
    ieee80211::client::{ClockPoisoned, StoppedInstallation},
    shared_radio::SharedRadioLease,
};
pub fn retain<T>(
    stopped: StoppedInstallation,
    obligation: ClockPoisoned,
    _lease: &mut SharedRadioLease<'_, T>,
) -> (StoppedInstallation, ClockPoisoned) {
    #[cfg(ui_reject)]
    obligation.release_clocks(_lease); // ui-error: E0599
    (stopped, obligation)
}
