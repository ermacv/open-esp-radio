//! Wi-Fi's reactions to the coexistence module.
//!
//! The vendor coexistence library calls back into Wi-Fi when a second radio
//! starts coexistence. This module is that callback's executor-independent
//! body over the shared [`RadioSystem`].

use oer_esp32s31_coex::CoexStatusType;
use oer_esp32s31_hal::shared_radio::PlatformClockProvider;
use oer_esp32s31_ieee80211::coex::WifiCoexActivity;
use oer_esp32s31_radio_runtime::RadioSystem;

use crate::roles::radio_channel::apply_coex_status;

/// Wi-Fi's coexistence start callback, for the lifetime of the radio.
///
/// When another radio starts coexistence, a Wi-Fi that has published no
/// status publishes its idle status, and the phases restart, as the vendor
/// `pm_on_coex_start` does.
///
/// SOURCE: complete pinned `libpp.a[pm.o]::pm_on_coex_start`.
pub async fn run_wifi_coex_start<P, C: PlatformClockProvider, T: oer_time::Timer>(
    radio: &RadioSystem<P, C, T>,
) -> ! {
    loop {
        radio.wifi_coex_started().await;
        let mut guard = radio.lock().await;
        if guard.coex_schedule().status_of(CoexStatusType::Wifi) == 0 {
            apply_coex_status(&mut guard, WifiCoexActivity::Idle);
        }
        guard.restart_coex_phases();
    }
}
