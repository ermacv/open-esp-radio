//! The facade's binding to this image's interrupt entries, callbacks and
//! engine, its terminal policy and the driver's task.
use oer_esp32s31_wifi as wifi;

use crate::fatal::Failure;

/// The image's terminal policy for a station construction error the facade
/// cannot return.
pub(crate) fn failure(error: wifi::InitializationError) -> ! {
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
        _ => crate::fail(Failure::Station),
    }
}

wifi::bind_station! {
    pub(crate) mod binding {
        wifi: crate::WifiToken,
        radio: crate::RadioToken,
        callbacks: crate::station::Callbacks,
        observer: oer_trace::Noop,
        driver_observer: oer_trace::Noop,
        sink: (),
        engine: oer_esp32s31_compose::crypto::KeyUnwrapEngine,
        events: crate::station::Events,
        fail: crate::station::radio::failure,
    }
}

pub(crate) async fn drive(driver: wifi::Driver<binding::Binding>) {
    driver.run().await;
}
