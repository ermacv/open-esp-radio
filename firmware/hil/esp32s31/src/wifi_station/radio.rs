//! Image binding and task spawning through the chip station facade.
use super::{observation, station};
use oer_esp32s31_wifi as wifi;
use oer_ieee80211_cipher::aes::SoftwareKeyUnwrap;
pub fn failure(error: wifi::InitializationError) -> ! {
    match error {
        wifi::InitializationError::MembershipUnavailable => {
            oer_espressif_staged_runtime::fatal::boot_failure::<oer_esp32s31_hal::platform::Port>(
                "station-membership",
                || (),
            )
        }
        wifi::InitializationError::Request(request) => {
            oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, _>(
                request,
            )
        }
        wifi::InitializationError::Phy(fault) => oer_esp32s31_compose::phy_fault::reset(fault),
        _ => crate::fail(crate::fatal::Failure::RootInitialization),
    }
}

wifi::bind_station! {pub(crate) mod binding{wifi:crate::WifiToken,radio:crate::RadioToken,callbacks:super::station::Callbacks,observer:super::observation::StationObserver,driver_observer:super::observation::DriverObserver,engine:SoftwareKeyUnwrap,events:super::station::ProductEvents,fail:super::failure}}
pub async fn drive(driver: wifi::Driver<binding::Binding>) {
    driver.run().await;
}
