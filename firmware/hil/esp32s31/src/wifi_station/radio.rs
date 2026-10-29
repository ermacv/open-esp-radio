//! Image binding and task spawning through the chip station facade.
use super::{DriverObserver, StationObserver, control};
use oer_esp32s31_wifi as wifi;
pub fn failure(error: wifi::InitializationError) -> ! {
    match error {
        wifi::InitializationError::MembershipUnavailable => {
            oer_espressif_staged_runtime::fatal::boot_failure::<oer_esp32s31_hal::platform::Port>(
                "station-membership",
                || (),
            )
        }
        wifi::InitializationError::Request(request) => {
            oer_esp32s31_compose::platform::reset_request(request)
        }
        wifi::InitializationError::Phy(fault) => fault.reset(),
        _ => crate::fail(crate::fatal::Failure::RootInitialization),
    }
}

wifi::bind_station! {pub(crate) mod binding{wifi:crate::WifiToken,radio:crate::RadioToken,callbacks:super::control::Callbacks,observer:super::StationObserver,driver_observer:super::DriverObserver,engine:oer_esp32s31_compose::crypto::WifiKeyUnwrap,events:super::control::ProductEvents,fail:super::failure}}
pub async fn drive(driver: wifi::Driver<binding::Binding>) {
    driver.run().await;
}
