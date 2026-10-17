#![no_std]
use oer_ieee80211_arbiter::{Arbiter, ArbiterEnd, PendingRetune};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub fn late_retune<'id, K: Ieee80211Capabilities>(
    mut arbiter: Arbiter<'id, K>,
    pending: PendingRetune<'id, K>,
) {
    let mut failed = arbiter.finish(ArbiterEnd::PortFailed);
    #[cfg(ui_reject)]
    arbiter.retuned(pending); // ui-error: E0382
    #[cfg(not(ui_reject))]
    failed.retune_ended(pending);
    drop(failed.poll_notice());
}
