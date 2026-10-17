//! phy register and hardware operations.

pub mod agc;

pub mod analog_i2c;

pub mod baseband;

pub mod clock;
#[cfg(target_arch = "riscv32")]
pub mod delay;

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
