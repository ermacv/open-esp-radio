#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! The handoff of the ESP32-S31 radio blocks from esp-hal to the radio HAL,
//! and the radio's platform clocks on top of esp-hal's clock control.
//!
//! esp-hal owns the SoC and everything before the handoff; the radio HAL owns
//! the radio blocks after it (docs/decisions.md, "Hardware ownership root").
//! [`take_radio`] consumes esp-hal's singletons of the blocks the radio owns
//! whole ([`RadioSingletons`]) and returns the HAL's [`RadioHardware`] root.
//! It also requires the adopted [`Psram`]: PSRAM adoption takes esp-hal's
//! never-released MPLL reference, so the radio's own MPLL requests never
//! cross zero (`registers/esp32s31/shared-words.toml`).
//!
//! Blocks where the radio owns only single words (PMU, HP_SYS_CLKRST,
//! LP_AON_CLK_RST, LP_PERICLKRST, the interrupt matrix) stay esp-hal's; each
//! shared word has one runtime writer, reviewed in `shared-words.toml`.
//!
//! Known limit of the guarantee: esp-hal's `X::regs()` is a safe function
//! that needs no singleton, so consuming a singleton excludes every driver
//! that takes it by value, but not a direct `regs()` call. This repository's
//! `clippy.toml` disallows `regs()` of the consumed peripherals.
//!
//! [`PlatformClocks`] implements the HAL's `PlatformClockProvider` over a
//! [`ClockBackend`]: on the chip, [`EspHalClocks`] maps each platform clock
//! to esp-hal's reference-counted `clock::ll` request and release. A
//! reference is given back only through the guard its request returned, so a
//! release can never run without its request. [`EspHal`] names that backend
//! itself, for an image that wraps it in its own [`ClockBackend`].
//!
//! [`RadioHardware`]: oer_esp32s31_hal::root::RadioHardware
//! [`Psram`]: esp_hal::psram::Psram

use core::marker::PhantomData;

use oer_esp32s31_hal::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockError, PlatformClockGuard,
    PlatformClockProvider,
};

#[cfg(feature = "esp32s31")]
mod esp_hal_handoff;

#[cfg(feature = "esp32s31")]
pub use esp_hal_handoff::{EspHal, EspHalClocks, RadioHandoffError, RadioSingletons, take_radio};

/// One platform's reference counts of the radio's platform clocks.
///
/// Its functions take no state: the platform's counts are global, which is
/// what lets a [`PlatformClockGuard`] give a reference back without a
/// borrow of its provider.
pub trait ClockBackend {
    /// Take one reference to `clock`.
    fn acquire(clock: PlatformClock);
    /// Give back one reference to `clock` that [`Self::acquire`] took.
    fn release(clock: PlatformClock);
    /// Record that a reference to `clock` stays held until a chip reset.
    fn abandon(clock: PlatformClock, how: PlatformClockAbandonment);
}

/// The radio's platform clock provider over the backend `B`.
pub struct PlatformClocks<B> {
    backend: PhantomData<fn() -> B>,
}

impl<B: ClockBackend> PlatformClocks<B> {
    /// A provider over `B`. Many providers may exist: the counts are the
    /// backend's.
    pub const fn new() -> Self {
        Self {
            backend: PhantomData,
        }
    }
}

impl<B: ClockBackend> Default for PlatformClocks<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: ClockBackend> core::fmt::Debug for PlatformClocks<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PlatformClocks")
    }
}

impl<B: ClockBackend> PlatformClockProvider for PlatformClocks<B> {
    fn acquire(&self, clock: PlatformClock) -> Result<PlatformClockGuard, PlatformClockError> {
        B::acquire(clock);
        Ok(PlatformClockGuard::new(clock, B::release, B::abandon))
    }
}

#[cfg(test)]
mod tests;
