//! Shared modem clocks, resets, power and channel-disjoint event routing.
#![forbid(unsafe_code)]
mod clock_device;
pub mod coex;
pub mod coex_external;
mod low_power_phy;
pub(crate) mod ops;
pub mod ownership;
pub mod platform;
pub mod shared_clock;
pub mod syscon;
pub mod values;
pub use clock_device::ModemClockDevice;
pub use coex::{CoexTimerBankRegisters, CoexTimerRegister};
pub use coex_external::{ExternalCoexPriority, ExternalCoexRole, ExternalCoexWires};
pub use ownership::SharedRadioRegisters;
pub use platform::{PlatformClockPowerObservation, PlatformPllSourceBaseline, WifiPowerBaseline};
pub use shared_clock::*;
pub use syscon::*;

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK. 40 MHz / (399 + 1) = 100 kHz.
pub const BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER: values::ModemLowPowerClockDivider =
    match values::ModemLowPowerClockDivider::new(
        oer_esp32s31_knowledge::modem_clock::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER,
    ) {
        Some(value) => value,
        None => panic!("the sourced divider fits its field"),
    };

const fn field_byte<const WIDTH: u32>(value: oer_mmio::Bits<WIDTH>) -> u8 {
    const {
        assert!(WIDTH <= 8);
    }
    value.get().to_le_bytes()[0]
}
const fn field_halfword<const WIDTH: u32>(value: oer_mmio::Bits<WIDTH>) -> u16 {
    const {
        assert!(WIDTH <= 16);
    }
    let [a, b, _, _] = value.get().to_le_bytes();
    u16::from_le_bytes([a, b])
}
// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT. State-map flags within a four-bit field.
const fn map_has(value: oer_mmio::Bits<4>, flag: u32) -> bool {
    value.get() & flag != 0
}
