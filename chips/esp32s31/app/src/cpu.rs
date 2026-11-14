//! What a hart counts of its own work, read at a window's two ends: its
//! cycles and retired instructions, the boot hart's L1 cache counters, and
//! each hart's idle account, which its executor's wait for an interrupt
//! keeps at the wait's two edges. Nothing here hooks a poll or a frame, and
//! every build has it.

use core::marker::PhantomData;

use oer_chips_esp32s31_hal::peripherals::cache_counters::{Buses, CacheCounters};

pub use oer_chips_esp32s31_hal::peripherals::cache_counters::{
    DataBus, Geometry as CacheGeometry, InstructionBus, Level as CacheLevel,
    Snapshot as CacheSnapshot,
};
pub use oer_chips_family_platform_executor::{Concurrent, IdleReading};

/// The boot hart's counters and the L1 cache's. It stays on the boot hart.
pub struct Cpu0 {
    cache: CacheCounters,
    _hart: PhantomData<*mut ()>,
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

    /// The boot hart's counters and the cache's; with `start`, clear and run
    /// every bus's cache counters first.
    pub fn counters(&mut self, start: bool) -> HartCounters {
        if start {
            self.cache.start(Buses::ALL);
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
}

/// Hart `hart`'s idle account now: its microseconds in the wait for an
/// interrupt since boot, and its wakes. `None` for no such hart.
///
/// # Errors
/// [`Concurrent`]: the hart wrote its account meanwhile; read again.
pub fn idle(hart: usize) -> Option<Result<IdleReading, Concurrent>> {
    oer_chips_family_platform_executor::IDLE
        .get(hart)
        .map(oer_chips_family_platform_executor::IdleAccount::read)
}
