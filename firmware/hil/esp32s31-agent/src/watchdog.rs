//! HIL-owned engineering budgets, not qualified production limits.
use core::num::NonZeroU32;
use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use static_cell::StaticCell;

static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
const STARTUP: DeadlineBudget = DeadlineBudget::from_micros(NonZeroU32::new(5_000_000).unwrap());
// MAC/RX/IRQ quiescence, Wi-Fi release and RF close.
const SHUTDOWN: DeadlineBudget = DeadlineBudget::from_micros(NonZeroU32::new(1_000_000).unwrap());

pub(super) fn init(timg: esp_hal::peripherals::TIMG1<'static>) -> &'static DeadlineWatchdog {
    SERVICE.init(DeadlineWatchdog::new(timg))
}

#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(super) fn wifi(
    service: &'static DeadlineWatchdog,
) -> &'static oer_esp32s31_ieee80211_system::WatchdogConfig {
    static CONFIG: StaticCell<oer_esp32s31_ieee80211_system::WatchdogConfig> = StaticCell::new();
    CONFIG.init(oer_esp32s31_ieee80211_system::WatchdogConfig::new(
        service, STARTUP, SHUTDOWN,
    ))
}
