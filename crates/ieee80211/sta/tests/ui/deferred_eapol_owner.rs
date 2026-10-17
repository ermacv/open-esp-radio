#![no_std]
use oer_ieee80211_sta::machine::{PendingGroupM1, PendingM3};

pub fn pairwise(pending: PendingM3) {
    let _frame = pending.into_frame();
    #[cfg(ui_reject)]
    let _again = pending.into_frame(); // ui-error: E0382
}

pub fn group(pending: PendingGroupM1) {
    let _frame = pending.into_frame();
    #[cfg(ui_reject)]
    let _again = pending.into_frame(); // ui-error: E0382
}
