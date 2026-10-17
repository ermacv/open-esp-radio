//! Owned ESP32-S31 cold-PHY prelude register leaves.
//!
//! This module contains only semantic operations used before and around the
//! RF initializer. Numeric MMIO identities stay in the generated PAC.

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_pac_mut};

/// Configure the fixed ESP32-S31 40 MHz crystal-derived tick value.
///
/// Complete pinned `libphy.a[phy_init.o]::phy_get_xtal_freq`, size `0x40`,
/// replaces the six-bit target with `frequency_mhz - 1`. ESP32-S31's public
/// chip contract fixes the crystal at 40 MHz. The shared route-owned PAC
/// performs one preserving generated transaction with no platform side door.
#[cfg(target_arch = "riscv32")]
pub fn configure_fixed_xtal_40mhz(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_fixed_xtal_40mhz_tick();
}

/// Sample the full-width counter used by the SDM-stability deadline.
///
/// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76`, size
/// `0x4a`, samples this word before and after each PHY-I2C read and compares
/// their wrapping unsigned difference with 9,999. This method performs one
/// read; deadline arithmetic and retry ownership stay in the transition.
#[cfg(target_arch = "riscv32")]
pub fn sample_sdm_deadline_counter(registers: &mut impl SharedPhyAccess) -> u32 {
    let registers = phy_pac_mut(registers);
    registers.sample_sdm_deadline_counter()
}
