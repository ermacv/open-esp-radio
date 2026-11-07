//! Stable runtime-clock conversion and machine-cycle samples for PHY settles.

/// The already adopted CPU clock, without a new clock lease or delay policy.
pub struct SettleClock;

impl oer_chips_family_phy::settle::SettleClock for SettleClock {
    fn cycles(&self, micros: u32) -> Option<u32> {
        crate::system::boot::ClockRegisters::short_settle_cycles(micros)
    }
    fn cycle32(&mut self) -> u32 {
        oer_portable_riscv::cycle32()
    }
}
