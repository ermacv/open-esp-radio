#![no_std]
use oer_portable_aes_software::SoftwareAes128;
use oer_protocols_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
pub fn transform(association: &CcmpAssociation, key: &Ccmp128Key, frame: &mut [u8]) {
    #[cfg(ui_reject)]
    let _result = association.seal(&mut SoftwareAes128, &[0; 16], frame); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _result = association.seal(&mut SoftwareAes128, key, frame);
}
