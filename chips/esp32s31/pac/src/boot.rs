//! Bootstrap acquisition and ownership parts; operations live in the HAL.

use crate::generated::soc::peripheral_ownership::{
    Claims, PeripheralPartitions, SocPeripherals, partition,
};

mod cache;
mod clock;
mod flash;
mod psram;

pub use cache::CacheRegisters;
pub use clock::ClockRegisters;
pub use flash::FlashRegisters;
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
