#![no_std]
use oer_ieee80211_arbiter::{Arbiter, PendingRetune};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub fn complete<'id, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K>,
    pending: PendingRetune<'id, K>,
) {
    arbiter.retuned(pending);
    #[cfg(ui_reject)]
    arbiter.retuned(pending); // ui-error: E0382
}
