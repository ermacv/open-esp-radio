//! Shared modem clocks, resets, power and channel-disjoint event routing.
#![forbid(unsafe_code)]
mod clock_device;
mod low_power_phy;
pub(crate) mod ops;
#[cfg(test)]
mod ops_tests;
pub mod ownership;
pub mod platform;
pub mod shared_clock;
pub mod syscon;
pub mod values;
pub use super::coex::timer_bank::CoexGrantTimerRegisters;
pub use clock_device::ModemClockDevice;
pub use ownership::{ForeignWifiMacWords, SharedRadioRegisters};
pub use platform::{PlatformClockPowerObservation, PlatformPllSourceBaseline, WifiPowerBaseline};
pub use shared_clock::*;
pub use syscon::*;

const fn field_byte<const WIDTH: u32>(value: oer_chips_esp32s31_pac::Bits<WIDTH>) -> u8 {
    const {
        if WIDTH > 8 {
            panic!("build-time check: WIDTH <= 8")
        };
    }
    value.get().to_le_bytes()[0]
}
// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT. State-map flags within a four-bit field.
