#![no_std]
use oer_ieee80211_core_roles_sta::twt::{IndividualTwtTeardownReady, IndividualTwtTransmittingFlow};
use oer_time::Instant;

pub fn teardown(ready: IndividualTwtTeardownReady<'_>, now: Instant) {
    ready.queue_teardown(now);
}

pub fn transmitting(_flow: IndividualTwtTransmittingFlow<'_>, _now: Instant) {
    #[cfg(ui_reject)]
    _flow.queue_teardown(_now); // ui-error: E0599
}
