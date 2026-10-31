#![no_std]
use oer_ieee80211_core_arbiter::{AbsencePlace, Arbiter, ChannelRequest, Lease, NoReservation};
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_radio_port::{Child, Root};
use oer_time::Instant;

pub fn request<'id, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K, NoReservation, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    home: &Lease<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    now: Instant,
    request: ChannelRequest<K>,
    own: AbsencePlace<'id, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    sibling: AbsencePlace<'id, Child<Root<oer_radio_port::LocalId<'id>>, 1>>,
) {
    #[cfg(ui_reject)]
    let _answer = arbiter.request_absence(now, request, home, sibling); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _answer = arbiter.request_absence(now, request, home, own);
}
