//! Preserve the first driver failure across cores; convert only at report time.
#[cfg(feature = "station-exit-evidence")]
use core::cell::Cell;
#[cfg(feature = "station-exit-evidence")]
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(feature = "station-exit-evidence")]
use oer_esp32s31_ieee80211_system::AccessPointRxRejection;

#[cfg(feature = "station-exit-evidence")]
static FIRST: Mutex<CriticalSectionRawMutex, Cell<Option<AccessPointRxRejection>>> =
    Mutex::new(Cell::new(None));

#[cfg(feature = "station-exit-evidence")]
pub(super) fn observe(record: Option<AccessPointRxRejection>) {
    if let Some(record) = record {
        FIRST.lock(|first| {
            if first.get().is_none() {
                first.set(Some(record));
            }
        });
    }
}

pub(super) fn snapshot() -> Option<oer_hil_protocol::wifi::WifiRxRejection> {
    #[cfg(feature = "station-exit-evidence")]
    {
        FIRST
            .lock(Cell::get)
            .map(oer_hil_esp32s31_telemetry::rx_rejection::evidence)
    }
    #[cfg(not(feature = "station-exit-evidence"))]
    {
        None
    }
}

#[cfg(feature = "station-exit-evidence")]
pub(super) fn reset() {
    FIRST.lock(|first| first.set(None));
}
