//! What a hart counts of its own work, read at a window's two ends: its
//! cycles and retired instructions and the boot hart's L1 cache counters,
//! in every build (unread, they cost nothing), and with the `cpu-load`
//! instrumentation class (`oer_cpu_load`) its idle account, which its
//! executor's wait for an interrupt keeps at the wait's two edges. Nothing
//! here hooks a poll or a frame. The entry hands `Cpu0` to the boot
//! (`Peripherals::cpu`) and `Cpu1` to the group on hart 1
//! (`GroupPeripherals::cpu`); each stays on its hart.

use core::marker::PhantomData;

use oer_chips_esp32s31_hal::peripherals::cache_counters::{Buses, CacheCounters};

pub use oer_chips_esp32s31_hal::peripherals::cache_counters::{
    DataBus, Geometry as CacheGeometry, InstructionBus, Level as CacheLevel,
    Snapshot as CacheSnapshot,
};
#[cfg(oer_cpu_load)]
pub use oer_chips_family_platform_executor::IdleReading;

/// The boot hart's counters and the L1 cache's. It stays on the boot hart.
pub struct Cpu0 {
    cache: CacheCounters,
    _hart: PhantomData<*mut ()>,
}

/// What a reading of [`Cpu0::counters`] does to the cache counters first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheRun {
    /// Clear every bus's counters and start them.
    Restart,
    /// Leave them running as they are.
    Keep,
}

/// The boot hart's counters at one instant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HartCounters {
    pub cycles: u64,
    pub instructions: u64,
    pub cache: CacheSnapshot,
    pub cache_geometry: CacheGeometry,
}

impl Cpu0 {
    pub(crate) const fn new(cache: CacheCounters) -> Self {
        Self {
            cache,
            _hart: PhantomData,
        }
    }

    /// The boot hart's counters and the cache's; with [`CacheRun::Restart`],
    /// clear and run every bus's cache counters first.
    pub fn counters(&mut self, run: CacheRun) -> HartCounters {
        match run {
            CacheRun::Restart => self.cache.start(Buses::all()),
            CacheRun::Keep => {}
        }
        let cycles = oer_portable_riscv::cycle64();
        let instructions = oer_portable_riscv::instret64();
        HartCounters {
            cycles,
            instructions,
            cache: self.cache.snapshot(),
            cache_geometry: self.cache.geometry(),
        }
    }

    /// The boot hart's idle account now: its microseconds in the wait for an
    /// interrupt since boot, and its wakes. Read on the hart that writes it,
    /// it never overlaps the write.
    #[cfg(oer_cpu_load)]
    pub fn idle(&mut self) -> IdleReading {
        let [hart0, ..] = &oer_chips_family_platform_executor::IDLE;
        hart0.read()
    }
}

/// Hart 1's counters. It stays on hart 1.
#[cfg(feature = "group")]
pub struct Cpu1 {
    _hart: PhantomData<*mut ()>,
}

/// A hart's own cycle and retired instruction counters at one instant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HartClock {
    pub cycles: u64,
    pub instructions: u64,
}

#[cfg(feature = "group")]
impl Cpu1 {
    pub(crate) const fn new() -> Self {
        Self { _hart: PhantomData }
    }

    /// Hart 1's own counters now.
    pub fn counters(&mut self) -> HartClock {
        HartClock {
            cycles: oer_portable_riscv::cycle64(),
            instructions: oer_portable_riscv::instret64(),
        }
    }

    /// Hart 1's idle account now, as [`Cpu0::idle`] reads the boot hart's.
    #[cfg(oer_cpu_load)]
    pub fn idle(&mut self) -> IdleReading {
        let [_, hart1, ..] = &oer_chips_family_platform_executor::IDLE;
        hart1.read()
    }
}
