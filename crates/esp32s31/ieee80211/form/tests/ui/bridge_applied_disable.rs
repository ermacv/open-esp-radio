#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{bridge::{Applied,Replies},PhaseCommand};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
#[cfg(ui_reject)]
type Receipt<'id,Caps> = PhaseCommand<'id,Caps,(),(),(),()>;
#[cfg(not(ui_reject))]
type Receipt<'id,Caps> = Applied<PhaseCommand<'id,Caps,(),(),(),()>>;

use oer_esp32s31_ieee80211_form::scope::commands::Enable;
pub fn disable<'id,Caps:Ieee80211Capabilities>(receiver:&Replies<'_,'id,Caps,(),(),(),(),4,9>,enable:Enable<'id>,reply:Receipt<'id,Caps>) {
    let _ = receiver.disabled(enable,reply); // ui-error: E0308
}
