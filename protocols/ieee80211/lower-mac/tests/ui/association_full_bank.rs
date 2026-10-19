#![no_std]
use oer_ieee80211_lower_mac::{Station, association::{Association, PairwiseKeySlot, GroupKeySlot, VacantKeyBank, end_association}};
pub fn end<'id>(owner: Association<'id, Station, 2, 2>, pairwise: [PairwiseKeySlot<'id, Station, 2, 2>; 2], group: [GroupKeySlot<'id, Station, 2, 2>; 2]) {
    let [first, second] = pairwise;
    #[cfg(ui_reject)]
    let bank = VacantKeyBank::new([first], group); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let bank = VacantKeyBank::new([first, second], group);
    let _vacant = end_association(owner, bank);
}
