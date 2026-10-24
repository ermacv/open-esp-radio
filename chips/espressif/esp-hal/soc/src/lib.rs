#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-S31 SoC services backed by esp-hal peripheral witnesses.
//!
//! Radio registers belong to `oer-esp32s31-pac`. This crate owns
//! esp-hal singleton witnesses for non-radio platform blocks. Cache and flash
//! modules provide typed register operations; [`handoff`] transfers the
//! eFuse, entropy, reset-cause and watchdog blocks to the chip PAC. These services
//! are separate from the generated radio PAC and from radio protocol policy.

mod cache;
#[cfg(feature = "esp32s31")]
mod flash;
#[cfg(feature = "esp32s31")]
pub mod handoff;
#[cfg(any(test, feature = "esp32s31"))]
mod once;

#[cfg(feature = "esp32s31")]
pub use cache::performance::L1CachePerformanceCounters;
pub use cache::performance::{
    L1CacheBusSnapshot, L1CacheCounterEnable, L1CachePerformanceSnapshot,
};
#[cfg(feature = "esp32s31")]
pub use flash::mmu::{FLASH_XIP_END, FLASH_XIP_START, FlashMmu};
