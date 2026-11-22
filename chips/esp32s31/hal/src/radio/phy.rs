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
