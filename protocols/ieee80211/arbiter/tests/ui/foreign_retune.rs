#![no_std]
use oer_ieee80211_arbiter::{Arbiter, PendingRetune};
use oer_ieee80211_port::Ieee80211Capabilities;

pub fn complete<'id, 'foreign, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K>,
    own: PendingRetune<'id, K>,
    foreign: PendingRetune<'foreign, K>,
) {
    #[cfg(ui_reject)]
    arbiter.retuned(foreign); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    arbiter.retuned(own);
}
