#![no_std]
use oer_esp32s31_ieee80211_form::scope::DriverStopped;
use oer_esp32s31_ieee80211_form::scope::commands::bridge::{Executor, Held};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

pub struct Dma;

// Frames return to their pools only with the stop's DriverStopped proof; no
// other value can stand in for proven DMA quiescence.
#[cfg(not(ui_reject))]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, Dma>,
    executor: &mut Executor<Dma, 4, 9>,
    stopped: &DriverStopped<'_, Dma>,
) {
    let _ = held.finish(executor, stopped);
}

#[cfg(ui_reject)]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, Dma>,
    executor: &mut Executor<Dma, 4, 9>,
    stopped: &(),
) {
    let _ = held.finish(executor, stopped); // ui-error: E0308
}
