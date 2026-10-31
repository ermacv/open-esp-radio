#![no_std]
use oer_ieee80211_core_roles_sta::receive::Management;

pub fn management(frame: &[u8]) -> Option<Management<'_>> {
    #[cfg(ui_reject)]
    return oer_ieee80211_wire::frame::DecodedHeader::decode(frame).map(Management); // ui-error: E0423
    #[cfg(not(ui_reject))]
    match oer_ieee80211_core_roles_sta::receive::classify(frame) {
        Ok(oer_ieee80211_core_roles_sta::receive::Received::Management(management)) => Some(management),
        _ => None,
    }
}
