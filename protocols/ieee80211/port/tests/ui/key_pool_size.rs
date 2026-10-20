#![no_std]
use oer_ieee80211_port::Station;
use oer_ieee80211_port::association::{
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
