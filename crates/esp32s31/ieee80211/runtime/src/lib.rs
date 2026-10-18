#![no_std]
#![forbid(unsafe_code)]

//! Embassy-specific composition for the ESP32-S31 Wi-Fi backend.
//!
//! Core radio, PHY and MAC crates remain executor-neutral. This crate owns
//! wakeups, cooperative async TX access and the bridge from pinned network
//! frames to ESP32-S31 Wi-Fi DMA.

#[cfg(test)]
extern crate std;

#[cfg(target_arch = "riscv32")]
pub mod coex;
pub mod datapath;
pub mod diagnostics;
pub mod lower_mac;
pub mod mac_clock;
pub mod roles;
mod time;

/// Generation `jumps` of the port's radio-clock relation, for tests that
/// name generations by number.
#[cfg(any(test, all(feature = "test-support", not(target_pointer_width = "32"))))]
pub(crate) fn clock_generation(jumps: u32) -> oer_ieee80211_lower_mac::Ieee80211ClockGeneration {
    let mut source = oer_ieee80211_lower_mac::GenerationSource::new(0);
    for _ in 0..jumps {
        source.jump();
    }
    source.current()
}
