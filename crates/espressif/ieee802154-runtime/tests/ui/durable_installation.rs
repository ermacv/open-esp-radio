#![no_std]
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_espressif_ieee802154_engine::{ll::Ieee802154LowLevel, pib::Ieee802154PibDefaults};
use oer_espressif_ieee802154_runtime::{
    Ieee802154Random, Ieee802154Runtime, Ieee802154RuntimeParts,
};
use oer_time::Timer;

pub fn install<M, H, T, R>(
    #[cfg(ui_reject)] runtime: &IRuntime<'static, M, H, T, R, 4>,
    #[cfg(not(ui_reject))] runtime: &'static IRuntime<'static, M, H, T, R, 4>,
    parts: Ieee802154RuntimeParts<'static, H>,
    random: R,
) where
    M: RawMutex + 'static,
    H: Ieee802154LowLevel + 'static,
    T: Timer + 'static,
    R: Ieee802154Random + 'static,
{
    let _ = runtime.install::<()>(parts, random, Ieee802154PibDefaults::default()); // ui-error: E0521
}
