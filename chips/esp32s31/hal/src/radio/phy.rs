//! phy register and hardware operations.

pub mod agc;

pub mod analog_i2c;

pub mod baseband;

pub mod clock;

pub mod frequency;

pub mod i2c;

pub mod iq_estimator;

pub mod memory;

pub mod pbus;

pub mod power_detector;

/// The radio's power, clock and reset words for a diagnostic record.
pub mod power_words;

pub mod prelude;

pub(crate) mod registration;

pub(crate) mod restore;

pub mod rx_dco;

pub mod temperature;

#[cfg(test)]
mod ops_tests;

/// Owned physical register sets for PHY operations.
pub mod registers;
pub use registers::RadioPhyRegisters;

#[cfg(target_arch = "riscv32")]
pub(crate) mod sequence;

/// `value` modulo four, a two-bit field's value: every residue is one of
/// its four constants. The PHY's one residue, of a ten-bit tone selector
/// and of a channel's bandwidth code.
pub(crate) const fn residue_of_four(value: u16) -> oer_portable_mmio::Bits<2> {
    match value % 4 {
        0 => oer_portable_mmio::Bits::constant::<0>(),
        1 => oer_portable_mmio::Bits::constant::<1>(),
        2 => oer_portable_mmio::Bits::constant::<2>(),
        _ => oer_portable_mmio::Bits::constant::<3>(),
    }
}
