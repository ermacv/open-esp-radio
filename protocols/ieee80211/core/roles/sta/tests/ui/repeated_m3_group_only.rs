#![no_std]
use oer_ieee80211_core_security_rsn::supplicant::RsnStaRetryInstall;

pub fn install(request: RsnStaRetryInstall) {
    #[cfg(ui_reject)]
    let _ = request.pairwise(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = request.group();
}
