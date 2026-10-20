#![no_std]
use oer_ieee80211_rsn::management_ccmp::ManagementCcmpReceiver;
use oer_ieee80211_wire::ccmp::Ccmp128Key;
pub fn receiver(key: Ccmp128Key) -> ManagementCcmpReceiver {
    let first = ManagementCcmpReceiver::new(key);
    #[cfg(ui_reject)]
    let second = ManagementCcmpReceiver::new(key); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = first;
    second
}
