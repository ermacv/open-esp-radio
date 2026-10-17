#![no_std]
use oer_ieee80211_lower_mac::{Station, AccessPoint, association::{Association, VacantKeyBank, end_association}};
pub fn end<'id>(owner: Association<'id, Station, 1, 1>, own: VacantKeyBank<'id, Station, 1, 1>, foreign: VacantKeyBank<'id, AccessPoint, 1, 1>) {
    #[cfg(ui_reject)]
    let _vacant = end_association(owner, foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _vacant = end_association(owner, own);
}
