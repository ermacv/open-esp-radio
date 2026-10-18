#![no_std]
use oer_esp32s31_ieee80211_form::scope::ScopeRoute;
pub fn return_route<P, E>(route: ScopeRoute<P, E>) -> P {
    let platform = route.into_platform();
    #[cfg(ui_reject)]
    let _second = route.into_platform(); // ui-error: E0382
    platform
}
