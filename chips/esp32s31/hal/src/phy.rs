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

/// Place a PHY engine code item in executable internal SRAM for performance.
///
/// This chip boundary owns section placement; PHY callers remain safe Rust.
/// The existing staged-runtime linker and startup copy `.hot.text.*` to SRAM.
/// This macro accepts functions only, never writable storage.
#[macro_export]
macro_rules! place_phy_engine_hot_path {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $($body:tt)*) => {
        $(#[$attribute])*
        // SAFETY: the S31 staged-runtime linker maps `.hot.text.*` into aligned
        // executable SRAM and startup copies it before execution. Only code
        // items are accepted; storage and its initialization are unchanged.
        #[cfg_attr(
            target_arch = "riscv32",
            unsafe(link_section = ".hot.text.open_radio_phy_engine")
        )]
        $visibility fn $name $($body)*
    };
}
#[cfg(test)]
mod ops_tests;
