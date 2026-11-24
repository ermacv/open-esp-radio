#![no_std]
use oer_protocols_ieee80211_core_security_rsn::supplicant::RsnStaGroupInstall;

pub fn install(request: RsnStaGroupInstall) {
    #[cfg(ui_reject)]
    let _ = request.pairwise(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = request.group();
}
