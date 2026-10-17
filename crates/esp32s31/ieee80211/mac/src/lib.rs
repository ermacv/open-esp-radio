#![no_std]
#![forbid(unsafe_code)]

//! Source-owned ESP32-S31 Wi-Fi MAC building blocks.
//!
//! Production builds contain no ESP-IDF ABI, vendor archive, allocator,
//! executor, or `esp-hal` dependency. Target code supplies volatile MMIO and
//! owns the interrupt binding; the state machines here stay host-testable.
//! The explicit `test-support` feature enables synthetic completion evidence
//! and DMA models on both host and RV32, including A-MPDU formatter buffers
//! and synthetic RX bindings. These APIs require hardware doubles. Production
//! builds validate real DMA addresses on every architecture.

#[cfg(test)]
extern crate std;

pub use oer_esp32s31_hal::types::{
    MacInterface, MacPti, MacRoleReceivePolicy, MacStaApReceivePlan, MacStaPolicyMode,
};

pub mod ap_policy;
pub mod ap_tsf;
pub mod capabilities;
pub mod channel_state;
pub mod coex_runtime;
mod cold;
pub mod crypto;
pub mod edca;
pub mod he;
pub mod init;
mod interface_address;
pub mod irq;
/// The 32-bit MAC local time on a 64-bit timeline.
pub mod local_time;
/// Conversions to and from the portable values of the lower-MAC port.
pub mod portable;
pub mod rx;
mod sniffer;
pub mod sta_ap_registers;
mod sta_link_policy;
pub mod tx;

/// Rate selection, receive observations and scheduling policy.
pub mod rate;

/// Generation `jumps` of the port's radio-clock relation, for tests that
/// name generations by number.
#[cfg(test)]
pub(crate) fn clock_generation(jumps: u32) -> oer_ieee80211_lower_mac::Ieee80211ClockGeneration {
    let mut source = oer_ieee80211_lower_mac::GenerationSource::new(0);
    for _ in 0..jumps {
        source.jump();
    }
    source.current()
}
