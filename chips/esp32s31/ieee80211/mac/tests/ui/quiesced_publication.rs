#![no_std]
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};
pub fn quiesce(mut active: ActiveEpoch<Moderated>) {
    #[cfg(not(ui_reject))]
    let _snapshot = active.service_mac();
    let inactive = active.quiesce();
    #[cfg(ui_reject)]
    let _snapshot = active.service_mac(); // ui-error: E0382
    drop(inactive);
}
