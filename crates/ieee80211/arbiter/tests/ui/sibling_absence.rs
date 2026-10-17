#![no_std]
use oer_ieee80211_arbiter::{AbsencePlace, Arbiter, ChannelRequest, Lease, NoReservation};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_radio_port::{Child, Root};
use oer_time::Instant;

pub fn request<'id, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K, NoReservation, Child<Root, 0>>,
    home: &Lease<'id, K, Child<Root, 0>>,
    now: Instant,
    request: ChannelRequest<K>,
    own: AbsencePlace<'id, Child<Root, 0>>,
    sibling: AbsencePlace<'id, Child<Root, 1>>,
) {
    #[cfg(ui_reject)]
    let _answer = arbiter.request_absence(now, request, home, sibling); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _answer = arbiter.request_absence(now, request, home, own);
}
