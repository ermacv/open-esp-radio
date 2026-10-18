#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::bridge::{Applied,Observation};
#[cfg(ui_reject)]
type Input<T> = T;
#[cfg(not(ui_reject))]
type Input<T> = Applied<T>;
pub fn receipt<T>(command:Input<T>)->Applied<T> {
    #[cfg(ui_reject)]
    { Applied { command,observation:Observation(0) } } // ui-error: E0451
    #[cfg(not(ui_reject))]
    { command }
}
