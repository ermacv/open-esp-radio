#![no_std]
use oer_esp32s31_ieee80211_form::scope::DriverStopped;
use oer_esp32s31_ieee80211_form::scope::commands::bridge::{Executor, Held};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub struct WifiDma;
pub struct OtherDma;

// Held bound for one DMA owner type finishes only on a stop proof about that
// type: a quiescent stop of other hardware cannot release its frames.
#[cfg(not(ui_reject))]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, WifiDma>,
    executor: &mut Executor<WifiDma, 4, 9>,
    stopped: &DriverStopped<'_, WifiDma>,
) {
    let _ = held.finish(executor, stopped);
}

#[cfg(ui_reject)]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, WifiDma>,
    executor: &mut Executor<WifiDma, 4, 9>,
    stopped: &DriverStopped<'_, OtherDma>,
) {
    let _ = held.finish(executor, stopped); // ui-error: E0308
}
