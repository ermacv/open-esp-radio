#![no_std]
use oer_ieee80211_core_arbiter::{Arbiter, NoReservation, PendingRetune};
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_radio_port::{Child, Root};

pub fn complete<'id, K: Ieee80211Capabilities>(
    arbiter: &mut Arbiter<'id, K, NoReservation, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    own: PendingRetune<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    sibling: PendingRetune<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 1>>,
) {
    #[cfg(ui_reject)]
    arbiter.retuned(sibling); // ui-error: E0308
    #[cfg(not(ui_reject))]
    arbiter.retuned(own);
}
