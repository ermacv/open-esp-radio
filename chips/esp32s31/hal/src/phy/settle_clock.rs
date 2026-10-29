//! Stable runtime-clock conversion and machine-cycle samples for PHY settles.

/// The already adopted CPU clock, without a new clock lease or delay policy.
pub struct SettleClock;

impl oer_espressif_phy::settle::SettleClock for SettleClock {
    fn cycles(&self, micros: u32) -> Option<u32> {
        crate::boot::ClockRegisters::short_settle_cycles(micros)
    }
    fn cycle32(&mut self) -> u32 {
        oer_riscv::cycle32()
    }
}
