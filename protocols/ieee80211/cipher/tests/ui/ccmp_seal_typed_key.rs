#![no_std]
use oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation;
use oer_ieee80211_wire::ccmp::Ccmp128Key;
pub fn transform(association: &CcmpAssociation, key: &Ccmp128Key, frame: &mut [u8]) {
    #[cfg(ui_reject)]
    let _result = association.seal(&[0; 16], frame); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _result = association.seal(key, frame);
}
