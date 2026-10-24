//! Platform values the image reads from blocks taken from esp-hal: the
//! factory identity from eFuse, entropy from the true random number
//! generator and why each HP core last left reset.
//!
//! Each owner is built from the PAC owner of its domain, which an esp-hal
//! adapter makes from its handoff. Nothing here writes a register.

pub use oer_esp32s31_knowledge::reset_cause::ResetCause;
pub use oer_esp32s31_pac::platform::{
    EfuseHandoff, EfuseRegisters, EntropyHandoff, EntropyRegisters, EntropyStatus, HpCore,
    ResetCauseHandoff, ResetCauseRegisters, RngClockPlacement, TrngPlacement,
};
pub use oer_espressif_efuse::{FactoryMac, Interface, MacAddress};
use oer_mmio::Placement;
use oer_time::{Clock, Duration, Instant};

use crate::settle::{Settle, SettleLimit, settle};

#[cfg(test)]
mod tests;

/// The chip's factory identity, read once from eFuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FactoryIdentity {
    mac: FactoryMac,
}

impl FactoryIdentity {
    /// Read the factory MAC fields; the eFuse owner is not needed again.
    pub fn read<P: Placement>(efuse: EfuseRegisters<P>) -> Self {
        Self {
            mac: efuse.factory_mac(),
        }
    }

    /// The factory (universally administered) MAC address.
    pub const fn base_mac(self) -> MacAddress {
        self.mac.base_mac()
    }

    /// The MAC extension bits, as the PHY calibration identity records them.
    pub const fn mac_extension(self) -> u16 {
        self.mac.mac_extension()
    }

    /// The address `interface` uses.
    pub const fn address(self, interface: Interface) -> MacAddress {
        interface.address(self.base_mac())
    }
}

/// The reset cause of each HP core, read once after boot: the words hold
/// until the next reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCauses {
    core0: ResetCause,
    core1: ResetCause,
}

impl ResetCauses {
    /// Read both cores' causes; the owner is not needed again.
    pub fn read<P: Placement>(registers: ResetCauseRegisters<P>) -> Self {
        Self {
            core0: ResetCause::from_code(registers.cause_code(HpCore::Core0)),
            core1: ResetCause::from_code(registers.cause_code(HpCore::Core1)),
        }
    }

    /// Why `core` last left reset.
    pub const fn of(self, core: HpCore) -> ResetCause {
        match core {
            HpCore::Core0 => self.core0,
            HpCore::Core1 => self.core1,
        }
    }
}

/// The shortest time between two reads of the generator's output, as `C`
/// measures it.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/rng/ll.rs:56-95` and ESP-IDF
/// 4d59230d `components/esp_hw_support/hw_random.c:43,89`: consecutive
/// reads are at least 16 APB clock cycles apart (`apb_cycle_wait_num`).
/// That is under one microsecond at every APB frequency of at least
/// 16 MHz; two microseconds of a clock counting whole microseconds
/// guarantee at least one.
pub const READ_GAP: Duration = Duration::from_micros(2);

/// The settle that waits out [`READ_GAP`], checked when the crate compiles.
const READ_GAP_SETTLE: SettleLimit = match SettleLimit::new(READ_GAP) {
    Some(limit) => limit,
    None => panic!("the read gap is a valid settle limit"),
};

/// Owns the true random number generator for the image's lifetime.
///
/// esp-hal enables and configures the generator in `esp_hal::init`; this
/// owner only reads it, spacing reads by [`READ_GAP`] with a named settle.
/// The time of the last read lives here, not in a global.
pub struct Entropy<C: Clock, P: Placement = TrngPlacement, Q: Placement = RngClockPlacement> {
    registers: EntropyRegisters<P, Q>,
    clock: C,
    last: Option<Instant>,
}

impl<C: Clock, P: Placement, Q: Placement> Entropy<C, P, Q> {
    /// Own the generator, timing reads with `clock`.
    pub const fn new(registers: EntropyRegisters<P, Q>, clock: C) -> Self {
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

    /// The generator's clock, sampling and health-test state.
    pub fn status(&self) -> EntropyStatus {
        self.registers.status()
    }
}
