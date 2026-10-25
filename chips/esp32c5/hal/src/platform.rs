//! Platform values the image reads from blocks taken from esp-hal: entropy
//! from the random number generator and why the HP core last left reset;
//! and the hart's interrupt controller ([`interrupt_controller`]), which the
//! interrupt table's router writes.
//!
//! Each owner is built from the PAC owner of its domain, which an esp-hal
//! adapter makes from its handoff.

/// The step-4 platform blocks (system timer, software interrupt lines,
/// interrupt matrix and CLIC) and the handoffs that claim them, for the
/// executor and the interrupt table.
pub use oer_esp32c5_pac::platform::{
    Clic, FromCpu0, FromCpu1, FromCpu2, FromCpu3, InterruptCore0, InterruptCore0Status,
    InterruptMatrixHandoff, InterruptMatrixPeripherals, SoftwareInterrupt0Handoff,
    SoftwareInterrupt0Peripherals, SoftwareInterrupt1Handoff, SoftwareInterrupt1Peripherals,
    SoftwareInterrupt2Handoff, SoftwareInterrupt2Peripherals, SoftwareInterrupt3Handoff,
    SoftwareInterrupt3Peripherals, SystemTimerHandoff, SystemTimerPeripherals, Systimer,
    SystimerAcknowledge, SystimerCounter, clic, from_cpu_0, from_cpu_1, from_cpu_2, from_cpu_3,
    interrupt_core0, interrupt_core0_status, systimer, systimer_acknowledge, systimer_counter,
};
pub use oer_esp32c5_pac::platform::{
    EntropyHandoff, EntropyRegisters, ResetCauseHandoff, ResetCauseRegisters, RngPlacement,
};
use oer_mmio::Placement;
use oer_time::settle::{Settle, SettleLimit, settle};
use oer_time::{Clock, Duration, Instant};

pub use interrupt_controller::{InterruptBlocks, InterruptController};
pub use reset_cause::ResetCause;

pub mod interrupt_controller;
mod reset_cause;
#[cfg(test)]
mod tests;

/// Why the HP core last left reset, read once after boot: the word holds
/// until the next reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCauses {
    core: ResetCause,
}

impl ResetCauses {
    /// Read the cause; the owner is not needed again.
    pub fn read<P: Placement>(registers: ResetCauseRegisters<P>) -> Self {
        Self {
            core: ResetCause::from_code(registers.cause_code()),
        }
    }

    /// Why the HP core last left reset.
    pub const fn core(self) -> ResetCause {
        self.core
    }
}

/// The shortest time between two reads of the generator's output, as `C`
/// measures it.
///
/// SOURCE: ESP-IDF 4d59230d `components/esp_hw_support/hw_random.c:43,89`:
/// consecutive reads are at least 16 APB clock cycles apart
/// (`APB_CYCLE_WAIT_NUM` on the ESP32-C5). That is under one microsecond
/// at every APB frequency of at least 16 MHz; two microseconds of a clock
/// counting whole microseconds guarantee at least one.
pub const READ_GAP: Duration = Duration::from_micros(2);

/// The settle that waits out [`READ_GAP`], checked when the crate compiles.
const READ_GAP_SETTLE: SettleLimit = match SettleLimit::new(READ_GAP) {
    Some(limit) => limit,
    None => panic!("the read gap is a valid settle limit"),
};

/// Owns the random number generator for the image's lifetime.
///
/// esp-hal does not enable the ESP32-C5 generator; this owner enables it as
/// ESP-IDF does when it is made, then reads it, spacing reads by
/// [`READ_GAP`] with a named settle. The time of the last read lives here,
/// not in a global.
pub struct Entropy<C: Clock, P: Placement = RngPlacement> {
    registers: EntropyRegisters<P>,
    clock: C,
    last: Option<Instant>,
}

impl<C: Clock, P: Placement> Entropy<C, P> {
    /// Enable the generator and own it, timing reads with `clock`.
    pub fn new(mut registers: EntropyRegisters<P>, clock: C) -> Self {
        registers.enable();
        Self {
            registers,
            clock,
            last: None,
        }
    }

    /// One random word, at least [`READ_GAP`] after the previous read.
    ///
    /// A named settle of at most [`READ_GAP`] waits out the gap; a
    /// monotonic clock always passes it, since the settle's last attempt
    /// runs after its limit, which is at least the gap since the last read.
    pub fn random(&mut self) -> u32 {
        if let Some(last) = self.last {
            let clock = &self.clock;
            settle(Settle::new(READ_GAP_SETTLE, clock), || {
                (clock.now().saturating_duration_since(last) >= READ_GAP).then_some(())
            })
            .expect("a monotonic clock passes the read gap by the settle's last attempt");
        }
        let word = self.registers.random_word();
        self.last = Some(self.clock.now());
        word
    }

    /// Fill `bytes` with random bytes, four from each word, little-endian.
    pub fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(4) {
            let word = self.random().to_le_bytes();
            for (byte, random) in chunk.iter_mut().zip(word) {
                *byte = random;
            }
        }
    }

    /// `N` random bytes.
    pub fn random_bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0; N];
        self.fill(&mut bytes);
        bytes
    }
}
