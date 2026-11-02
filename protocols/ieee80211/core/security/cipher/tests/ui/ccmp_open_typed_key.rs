#![no_std]
use oer_protocols_ieee80211_core_security_cipher::{aes::SoftwareAes128, ccmp_mpdu::CcmpAssociation};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
pub fn transform(association: &CcmpAssociation, key: &Ccmp128Key, frame: &mut [u8]) {
    #[cfg(ui_reject)]
    let _result = association.open(&mut SoftwareAes128, &[0; 16], frame); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _result = association.open(&mut SoftwareAes128, key, frame);
}
