#![no_std]
use oer_ieee80211_sta::ftm::{FtmRequestTransmission, FtmSession};
use oer_time::Instant;
pub fn complete<const N: usize>(first: FtmRequestTransmission<N>, mut second: FtmSession<N>, now: Instant) {
    // The original public counterexample applied first's copyable TX token
    // to second. Only the owned transmission now has completion authority.
    #[cfg(ui_reject)]
    let _result = second.complete_transmission(first, true, now); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _result = first.complete(true, now);
    let _second = second.cancel();
}
