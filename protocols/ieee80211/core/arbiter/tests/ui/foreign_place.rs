#![no_std]
use oer_ieee80211_core_arbiter::{Arbiter, ChannelRequest, LeasePlace};
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_time::Instant;

pub fn request<'id, 'foreign, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K>,
    now: Instant,
    request: ChannelRequest<K>,
    own: LeasePlace<'id>,
    foreign: LeasePlace<'foreign>,
) {
    #[cfg(ui_reject)]
    let _answer = arbiter.request(now, request, foreign); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let _answer = arbiter.request(now, request, own);
}
