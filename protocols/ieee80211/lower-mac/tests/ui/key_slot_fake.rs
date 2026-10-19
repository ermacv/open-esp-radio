#![no_std]
use oer_ieee80211_lower_mac::Station;
use oer_ieee80211_lower_mac::association::{AssociationPool, AssociationSlot, PairwiseKeySlot};

/// A slot a backend would declare to claim a key pool it does not mint.
pub struct Declared;

#[cfg(ui_reject)]
impl AssociationSlot for Declared { // ui-error: E0277
    type Pool = AssociationPool<Station, 8, 8>;
}

fn pool<S: AssociationSlot>() {}

pub fn minted() {
    pool::<PairwiseKeySlot<'static, Station, 2, 2>>();
}
