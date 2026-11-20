#![no_std]
use oer_protocols_ieee80211_core_roles_sta::ftm::{FtmRequestTransmission, FtmSession};
use oer_portable_time::Instant; use oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome;
pub fn complete<const N: usize>(first: FtmRequestTransmission<N>, mut second: FtmSession<N>, now: Instant) {
    // The original public counterexample applied first's copyable TX token
    // to second. Only the owned transmission now has completion authority.
    #[cfg(ui_reject)]
    let _result = second.complete_transmission(first, MpduOutcome::Acknowledged, now); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _result = first.complete(MpduOutcome::Acknowledged, now);
    let _second = second.cancel();
}
