//! The blocks the bootstrap brings the chip up with, claimed from the soc
//! partition, and their implementations of the `oer-espressif-boot`
//! register traits.
//!
//! The bootstrap is its own ELF and its own ownership epoch: it claims the
//! soc partition once, from [`BootHandoff`], and its owners end with it when
//! it jumps to stage two. Each owner implements the register traits with
//! the typed fields, one read-modify-write per call, so every other bit of a
//! word, reserved ones included, is kept; a command the hardware clears is
//! started only through its generated start, which refuses while one runs.
//! The traits' calls need an `oer_espressif_boot::Access`, which only that
//! crate's sequences make.
//!
//! - [`ClockRegisters`]: the CPU root and dividers, the CPU PLL, the clock
//!   update, the flash clock, and the memory PLL with the PSRAM's supply;
//! - [`CacheRegisters`]: fetch suspension and the sync engine;
//! - [`FlashRegisters`]: the SPI1 command port, the flash timing words and
//!   the flash MMU;
//! - [`PsramRegisters`]: the PSRAM controllers, their command port and the
//!   PSRAM MMU.
//!
//! The PLLs' power in the PMU's active state (`HP_ACTIVE_HP_CK_POWER`) is the
//! radio PMU block's word; its owner implements
//! `oer_espressif_boot::clock::ActivePllPower`.
//!
//! ```compile_fail,E0061
//! # use oer_esp32s31_pac::boot::ClockRegisters;
//! # use oer_espressif_boot::clock::CpuClockRegisters;
//! fn raw_stage(mut clock: ClockRegisters) {
//!     clock.stage(oer_esp32s31_pac::boot::CpuRoot::Cpll, Default::default()); // only a boot sequence stages
//! }
//! ```

use crate::generated::soc::peripheral_ownership::{
    Claims, PeripheralPartitions, SocPeripherals, partition,
};

mod cache;
mod clock;
mod flash;
mod psram;

#[cfg(test)]
mod tests;

pub use cache::{CacheMap, CacheRegisters};
pub use clock::{BusDividers, ClockRegisters, CpllDividers, CpuRoot, MemDivider, PsramSupplyLevel};
pub use flash::{FlashRegisters, FlashTimingCandidate, FlashTimingWords};
pub use psram::PsramRegisters;

/// Proof that the bootstrap owns the chip's soc blocks.
#[must_use = "the boot owners need the consumed bootstrap ownership"]
pub struct BootHandoff(());

impl BootHandoff {
    /// Assert the bootstrap's unique, permanent ownership of the soc blocks.
    ///
    /// # Safety
    /// Call at most once, in the bootstrap ELF, before any other code
    /// writes a soc block's word: no esp-hal driver, ROM helper or other
    /// owner of those words runs in the bootstrap's epoch.
    #[allow(
        unsafe_code,
        reason = "the bootstrap proves exclusive soc-block ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The bootstrap's owners of the soc blocks it brings the chip up with.
///
/// The other soc blocks are claimed with them and dropped: the bootstrap
/// writes no other block's word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct BootBlocks {
    /// The CPU clock, its PLL and update, the flash clock, the memory PLL
    /// and the PSRAM's supply.
    pub clock: ClockRegisters,
    /// The L1 cache's fetch suspension and sync engine.
    pub cache: CacheRegisters,
    /// The flash command port, timing and MMU.
    pub flash: FlashRegisters,
    /// The PSRAM controllers, command port and MMU.
    pub psram: PsramRegisters,
}

impl BootBlocks {
    /// Claim the soc partition from the bootstrap's handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: BootHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `BootHandoff`'s constructor guarantees exclusive,
        // permanent ownership of the soc blocks in the bootstrap's epoch,
        // and it is consumed here, so the blocks are claimed once. The
        // generated `const` assertion beside the soc blocks checks that no
        // curated typed block lists their words.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_partitions(partition(claims))
    }

    fn from_partitions(partitions: PeripheralPartitions) -> Self {
        let SocPeripherals {
            cache,
            hp_alive_sys,
            hp_sys_clkrst,
            iomux_mspi_pin,
            lp_aon_clkrst,
            memspi2,
            memspi3,
            pmu,
            spi0,
            spi1,
            ..
        } = partitions.soc;
        Self {
            clock: ClockRegisters::new(hp_sys_clkrst, lp_aon_clkrst, pmu, hp_alive_sys),
            cache: CacheRegisters::new(cache),
            flash: FlashRegisters::new(spi0, spi1),
            psram: PsramRegisters::new(memspi2, memspi3, iomux_mspi_pin),
        }
    }
}
