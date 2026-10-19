#![no_std]
use oer_ieee80211_arbiter::{Arbiter, PendingRetune, RetuneRefused};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub fn refused<'id, K: Ieee80211Capabilities>(
    mut arbiter: Arbiter<'id, K>,
    pending: PendingRetune<'id, K>,
) {
    let outcome = arbiter.retune_refused(pending);
    #[cfg(ui_reject)]
    arbiter.want_access_point(); // ui-error: E0382
    match outcome {
        RetuneRefused::Return { mut arbiter, pending } => arbiter.retuned(pending),
        RetuneRefused::Failed(mut failed) => drop(failed.poll_notice()),
    }
}
