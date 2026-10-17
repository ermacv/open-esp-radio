#![no_std]
use oer_ieee80211_lower_mac::InFlight;

pub fn reorder<C, T, M>(first: &mut InFlight<C, T, M>, second: &mut InFlight<C, T, M>) {
    #[cfg(ui_reject)]
    core::mem::swap(&mut first.ticket, &mut second.ticket); // ui-error: E0616 // ui-error: E0616
    #[cfg(ui_reject)]
    core::mem::swap(&mut first.context, &mut second.context); // ui-error: E0616 // ui-error: E0616
    #[cfg(not(ui_reject))]
    core::mem::swap(first, second);
}
