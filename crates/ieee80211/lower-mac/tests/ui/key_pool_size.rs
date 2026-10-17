#![no_std]
use oer_ieee80211_lower_mac::Station;
use oer_ieee80211_lower_mac::association::{
    AssociationPool, AssociationSlot, GroupKeySlot, PairwiseKeySlot,
};

fn declared<S: AssociationSlot<Pool = AssociationPool<Station, 2, 2>>>() {}

pub fn minted() {
    #[cfg(ui_reject)]
    declared::<PairwiseKeySlot<'static, Station, 1, 4>>(); // ui-error: E0271
    #[cfg(not(ui_reject))]
    declared::<PairwiseKeySlot<'static, Station, 2, 2>>();
    declared::<GroupKeySlot<'static, Station, 2, 2>>();
}
