#![no_std]
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_espressif_ieee802154_engine::{ll::Ieee802154LowLevel, pib::Ieee802154PibDefaults};
use oer_espressif_ieee802154_runtime::{Ieee802154Random, Ieee802154Runtime, MacStoppedParts, RuntimeQueue};
use oer_time::Timer;

pub fn reinstall<M, H, T, R>(
    runtime: &'static Ieee802154Runtime<'static, M, H, T, R, 4>,
    parts: MacStoppedParts<'static, H>,
    fresh: RuntimeQueue,
    random: R,
) where M: RawMutex + 'static, H: Ieee802154LowLevel + 'static,
    T: Timer + 'static, R: Ieee802154Random + 'static,
{
    #[cfg(not(ui_reject))]
    let parts = {
        let (engine, hardware) = parts.into_parts();
        oer_espressif_ieee802154_runtime::Ieee802154RuntimeParts::new(engine, hardware, fresh)
    };
    #[cfg(ui_reject)]
    let _ = fresh;
    let _ = runtime.install::<()>(parts, random, Ieee802154PibDefaults::default()); // ui-error: E0308
}
