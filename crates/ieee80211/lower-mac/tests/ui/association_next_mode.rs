#![no_std]
use oer_ieee80211_lower_mac::{Station, association::{AssociationStart, VacantKeyBank, end_association}};
use oer_ieee80211_mac::security::Spp;
pub fn next<'id>(start: AssociationStart<'id, Station, 1, 1>, bank: VacantKeyBank<'id, Station, 1, 1>) {
    let first = start.begin(Spp::Negotiated);
    #[cfg(ui_reject)]
    let _second = start.begin(Spp::NotNegotiated); // ui-error: E0382
    #[cfg(not(ui_reject))]
    {
        let (next, _bank) = end_association(first, bank).into_parts();
        let _second = next.begin(Spp::NotNegotiated);
    }
}
