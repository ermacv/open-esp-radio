#![no_std]
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};
pub fn quiesce(active: ActiveEpoch<Moderated>) {
    let inactive = active.quiesce();
    #[cfg(ui_reject)]
    let _second = active.quiesce(); // ui-error: E0382
    let _setup = inactive.into_setup();
}
