//! The L1 cache owner: core 0's fetch suspension and the sync engine.

use oer_espressif_boot::{
    Access, Busy,
    cache::{
        CacheSyncRegisters, FetchSuspendRegisters, LineRange, SecondaryFetchRegisters, SyncCommand,
    },
};
use oer_mmio::{Bits, Fixed, Placement};

use oer_esp32s31_pac::{cache::CacheBoot, instances};

/// The caches a sync command acts on (`SYNC_MAP`).
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/esp_rom/esp32s31/include/esp32s31/rom/cache.h:160-166`
/// (data cache BIT(4), both instruction caches BIT(0) | BIT(1)) and
/// `components/soc/esp32s31/register/soc/cache_reg.h:1630-1640`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheMap {
    Data,
    BothInstructionCaches,
}

impl CacheMap {
    const fn code(self) -> Bits<6> {
        match self {
            Self::Data => Bits::constant::<0x10>(),
            Self::BothInstructionCaches => Bits::constant::<0x03>(),
        }
    }
}

/// The L1 cache's words for core 0's instruction-fetch suspension.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CacheRegisters<P: Placement = Fixed<{ instances::cache::BASE }>>(CacheBoot<P>);

impl CacheRegisters {
    /// Own the bootstrap's cache part.
    pub(crate) fn new(cache: CacheBoot) -> Self {
        Self(cache)
    }
}

/// Core 0's instruction-fetch suspension on the L1 cache: instruction bus
/// 0, instruction cache 0's autoload, preload and freeze, the sync and lock
/// engines' idle flags.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/register/soc/cache_reg.h:16-22`
/// (`L1_ICACHE_SHUT_IBUS0`), `:805-829` (`L1_ICACHE0_FREEZE_EN`, `_MODE`,
/// `_DONE`), `:1496-1520` (`LOCK_DONE`), `:1574-1620` (`SYNC_DONE`),
/// `:1672-1684` (`L1_ICACHE0_PRELOAD_DONE`), `:2049-2061`
/// (`L1_ICACHE0_AUTOLOAD_ENA`, `_DONE`); the pinned rev0 ROM's
/// `Cache_Suspend_L1_CORE0_ICache` uses exactly these fields
/// (`oer_espressif_boot::cache`).
impl<P: Placement> FetchSuspendRegisters for CacheRegisters<P> {
    fn shut_fetch(&mut self, _: Access<'_>, shut: bool) {
        self.0.modify_l1_icache_ctrl(|w| {
            w.set_l1_icache_shut_ibus0(shut);
        });
    }

    fn autoload_enabled(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_ena()
    }

    fn set_autoload(&mut self, _: Access<'_>, enabled: bool) {
        self.0.modify_l1_icache0_autoload_ctrl(|w| {
            w.set_l1_icache0_autoload_ena(enabled);
        });
    }

    fn autoload_done(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_done()
    }

    fn preload_done(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_preload_ctrl()
            .l1_icache0_preload_done()
    }

    fn sync_done(&self, _: Access<'_>) -> bool {
        self.0.read_sync_ctrl().sync_done()
    }

    fn lock_done(&self, _: Access<'_>) -> bool {
        self.0.read_lock_ctrl().lock_done()
    }

    fn select_freeze_mode_zero(&mut self, _: Access<'_>) {
        self.0.modify_l1_cache_freeze_ctrl(|w| {
            w.set_l1_icache0_freeze_mode(false);
        });
    }

    fn set_freeze(&mut self, _: Access<'_>, frozen: bool) {
        self.0.modify_l1_cache_freeze_ctrl(|w| {
            w.set_l1_icache0_freeze_en(frozen);
        });
    }

    fn freeze_done(&self, _: Access<'_>) -> bool {
        self.0.read_l1_cache_freeze_ctrl().l1_icache0_freeze_done()
    }
}

/// Each secondary hart's instruction bus to the cache: hart 1's is
/// `L1_ICACHE_SHUT_IBUS1`. The bootstrap opens it for a hart the image
/// declares, after it published the runtime's code.
///
/// SOURCE: ESP_IDF_FFF9895C_S31_CACHE_LL_BUSES in
/// `chips/esp32s31/registers/evidence/memory.toml` (`cache_ll_l1_enable_bus`).
impl<P: Placement> SecondaryFetchRegisters for CacheRegisters<P> {
    type Secondary = crate::platform::SecondaryHart;
    const SECONDARIES: &'static [crate::platform::SecondaryHart] =
        &crate::platform::Hart::SECONDARIES;

    fn index(hart: crate::platform::SecondaryHart) -> u32 {
        hart.hart().get() as u32
    }

    fn open_fetch(&mut self, _: Access<'_>, hart: crate::platform::SecondaryHart) {
        match hart {
            crate::platform::SecondaryHart::H1 => self.0.modify_l1_icache_ctrl(|w| {
                w.set_l1_icache_shut_ibus1(false);
            }),
        }
    }
}

/// The sync engine: the map, the line-aligned range, one command alone and
/// its completion.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/register/soc/cache_reg.h:1574-1660`
/// (`SYNC_CTRL` commands self-clear, `SYNC_DONE`, `SYNC_MAP`, `SYNC_ADDR`,
/// `SYNC_SIZE`); esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/cache.rs:13-61`.
impl<P: Placement> CacheSyncRegisters for CacheRegisters<P> {
    type Map = CacheMap;

    fn set_range(&mut self, _: Access<'_>, map: CacheMap, range: LineRange) {
        self.0.modify_sync_map(|w| {
            w.set_sync_map(map.code());
        });
        self.0.modify_sync_addr(|w| {
            w.set_sync_addr(range.start());
        });
        // The geometry's largest range fits the 28-bit size field.
        self.0.modify_sync_size(|w| {
            w.set_sync_size(Bits::truncate(range.size()));
        });
    }

    fn start(&mut self, _: Access<'_>, command: SyncCommand) -> Result<(), Busy> {
        match command {
            SyncCommand::Writeback => self.0.start_sync_ctrl_writeback_ena(),
            SyncCommand::Invalidate => self.0.start_sync_ctrl_invalidate_ena(),
        }
        .map_err(|_| Busy)
    }

    fn done(&self, _: Access<'_>) -> bool {
        self.0.read_sync_ctrl().sync_done()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> CacheRegisters<oer_mmio::Host<'a>> {
    /// The cache owner over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        oer_esp32s31_pac::cache::cache_host(words).map(|(_buses, cache)| Self(cache))
    }
}
