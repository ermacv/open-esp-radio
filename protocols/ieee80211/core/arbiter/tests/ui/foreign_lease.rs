#![no_std]
use oer_ieee80211_core_arbiter::{Arbiter, Lease};
use oer_ieee80211_contracts_port::Ieee80211Capabilities;

pub fn release<'id, 'foreign, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K>,
    own: Lease<'id, K>,
    foreign: Lease<'foreign, K>,
) {
    #[cfg(ui_reject)]
    arbiter.release(foreign); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    arbiter.release(own);
}
