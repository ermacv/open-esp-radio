#![no_std]
use oer_ieee80211_core_security_rsn::management_ccmp::ManagementCcmpReceiver;
use oer_ieee80211_contracts_security::Ccmp128Key;
pub fn receiver(key: Ccmp128Key) -> ManagementCcmpReceiver {
    #[cfg(ui_reject)]
    let receiver = ManagementCcmpReceiver::new(&[0; 16]); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let receiver = ManagementCcmpReceiver::new(key);
    receiver
}
