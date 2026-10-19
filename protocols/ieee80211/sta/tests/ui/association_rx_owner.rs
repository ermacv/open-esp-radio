#![no_std]
use oer_ieee80211_sta::join::association::StaAssociationRuntime;
use oer_time::Instant;

pub fn receive(runtime: StaAssociationRuntime, frame: &[u8], now: Instant) {
    let _terminal = runtime.observe_management_frame(frame, now);
    #[cfg(ui_reject)]
    let _again = runtime.observe_management_frame(frame, now); // ui-error: E0382
}
