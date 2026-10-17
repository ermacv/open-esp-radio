#![no_std]
use oer_ieee80211_arbiter::{Arbiter, Lease};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub fn release<'id, 'foreign, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K>,
    own: Lease<'id, K>,
    foreign: Lease<'foreign, K>,
) {
    #[cfg(ui_reject)]
    arbiter.release(foreign); // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    #[cfg(not(ui_reject))]
    arbiter.release(own);
}
