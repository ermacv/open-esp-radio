#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_radio_port::Quarantined;
use core::convert::Infallible;
use oer_esp32s31_ieee80211_form::scope::{ScopeReturn, WakePlan};
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};
pub fn quarantine(
    value: Quarantined<ScopeReturn<RadioRuntimeOwner, ActiveEpoch<Moderated>, (), Infallible, fn(&(), WakePlan)>>,
) {
    #[cfg(ui_reject)]
    let _lease = value.lease; // ui-error: E0609
    #[cfg(not(ui_reject))]
    drop(value);
}
