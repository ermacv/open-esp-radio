#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_form::scope::commands::bridge::{Bridged, Held};
use oer_esp32s31_ieee80211_form::scope::{ReturnedOwners, StoppedOwners};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;

type Radio = Bridged<RadioRuntimeOwner, 4, 9>;
type Stopped = StoppedOwners<ReturnedOwners<Radio, (), (), (), ()>>;

// The stop proof borrows the stopped owners: while it lives the hardware, and
// so its DMA owner, cannot be taken out and stepped or installed again.
#[cfg(not(ui_reject))]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, RadioRuntimeOwner>,
    mut owners: Stopped,
) -> Radio {
    let (executor, stopped) = owners.parts();
    let _ = held.finish(executor, &stopped);
    owners.into_owners().hardware
}

#[cfg(ui_reject)]
pub fn end<'id, Caps: Ieee80211Capabilities>(
    held: Held<'id, Caps, (), (), (), (), 2, 1, 4, 9, RadioRuntimeOwner>,
    owners: Stopped,
) -> Radio {
    let stopped = owners.stopped();
    let mut radio = owners.into_owners().hardware; // ui-error: E0505
    let _ = held.finish(radio.executor(), &stopped);
    radio
}
