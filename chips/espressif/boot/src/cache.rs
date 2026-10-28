//! Cache operations of the boot: code published to instruction fetch, and
//! instruction fetch suspended around a flash access.
//!
//! **Publication.** Code the boot copied through the data cache reaches
//! instruction fetch only after its lines are written back and the
//! instruction caches' copies invalidated. The range is widened outward to
//! whole cache lines; the data cache is written back the chip's number of
//! times, each pass waiting for the sync engine, and then, on a chip with
//! separate instruction caches, those are invalidated. The caller executes
//! `fence.i` before it fetches the range. The sync engine's commands are
//! mutually exclusive: each starts alone, once none runs. A range just
//! mapped is dropped from the data cache ([`invalidate_data`]).
//!
//! SOURCE(esp32s31): ESP-IDF 4d80c1b2
//! `components/esp_rom/esp32s31/include/esp32s31/rom/cache.h:31,45,160-166`
//! (64-byte lines, data map 0x10, instruction map 0x03);
//! `components/esp_rom/patches/esp_rom_cache_writeback_esp32p4_esp32s31.c:39-50`
//! (two writebacks); `components/soc/esp32s31/register/soc/cache_reg.h:1574-1630`
//! (commands self-clear and exclude each other; `SYNC_DONE` reads the end).
//! SOURCE(esp32c5): esp-hal 1f646c5 `esp-hal/src/soc/esp32c5/mod.rs:95-117`
//! (32-byte lines, one shared cache 0x10 written back twice).
//!
//! **Fetch suspension.** A flash access through the SPI1 command port must
//! not race instruction fetch from flash. The sequence replaces the ROM's
//! `Cache_Suspend_L1_CORE0_ICache` and `Cache_Resume_L1_CORE0_ICache` with
//! their effects, each unbounded ROM poll a named boot wait:
//!
//! 1. shut core 0's instruction bus to the cache;
//! 2. note whether autoload runs, stop it if so, and wait for autoload to
//!    be idle;
//! 3. wait for the preload, the sync engine and the lock engine to be idle;
//! 4. freeze the instruction cache (mode 0) and wait for the freeze, then
//!    thaw it and wait for the thaw;
//! 5. run the operation;
//! 6. open the instruction bus again and restart autoload if it ran.
//!
//! A wait that fails restores what the sequence changed before the failure
//! returns.
//!
//! SOURCE(esp32s31): pinned rev0 ROM (esp-rom-elfs 20260528, SHA-256
//! a52ad751...ab87), Blobray function records of
//! `Cache_Suspend_L1_CORE0_ICache` @0x2f8027dc and its default-table
//! callees `Cache_Suspend_L1_CORE0_ICache_Autoload` @0x2f801ca2,
//! `Cache_Freeze_L1_ICache0_Enable` @0x2f801d5c (argument 0),
//! `Cache_Freeze_L1_ICache0_Disable` @0x2f801d82;
//! `Cache_Resume_L1_CORE0_ICache` @0x2f8027c6 and
//! `Cache_Resume_L1_CORE0_ICache_Autoload` @0x2f801e9e. Fields:
//! ESP-IDF 4d80c1b2 `cache_reg.h:16-22` (`L1_ICACHE_SHUT_IBUS0`),
//! `:805-829` (`L1_ICACHE0_FREEZE_EN`/`_MODE`/`_DONE`), `:1496-1520`
//! (`LOCK_DONE`), `:1574-1620` (`SYNC_DONE`), `:1672-1684`
//! (`L1_ICACHE0_PRELOAD_DONE`), `:2049-2061` (`L1_ICACHE0_AUTOLOAD_ENA`/`_DONE`).

use oer_time::Clock;

use crate::{
    Access, BootFailure, Busy,
    wait::{BootDeadline, BootWait},
};

/// One sync-engine command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncCommand {
    /// Write dirty lines of the range back to memory.
    Writeback,
    /// Drop the range's lines.
    Invalidate,
}

/// The cache sync engine's range and command words.
pub trait CacheSyncRegisters {
    /// The chip's selection of caches a command acts on.
    type Map: Copy;
    /// Select the caches and the line-aligned range of the next command.
    fn set_range(&mut self, access: Access<'_>, map: Self::Map, range: LineRange);
    /// Start `command` alone, every other command bit zero, or [`Busy`]
    /// while a command runs.
    fn start(&mut self, access: Access<'_>, command: SyncCommand) -> Result<(), Busy>;
    /// Whether the last command has finished.
    fn done(&self, access: Access<'_>) -> bool;
}

/// A chip's cache geometry for publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheGeometry<Map> {
    /// The cache line, a power of two.
    pub line_bytes: u32,
    /// The largest range the sync engine's size field holds.
    pub max_range_bytes: u32,
    /// The data cache the code was written through.
    pub data: Map,
    /// How often the data cache is written back.
    pub writeback_passes: u8,
    /// The instruction caches to invalidate, on a chip whose instruction
    /// caches are separate from the data cache.
    pub instruction: Option<Map>,
}

/// A range widened to whole cache lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineRange {
    start: u32,
    size: u32,
}

impl LineRange {
    /// `start..start + len` widened outward to `line_bytes` (a power of
    /// two): refused when empty, when its end overflows and when it exceeds
    /// `max_bytes`.
    pub const fn cover(
        start: u32,
        len: u32,
        line_bytes: u32,
        max_bytes: u32,
    ) -> Result<Self, CacheFailure> {
        if len == 0 {
            return Err(CacheFailure::EmptyRange);
        }
        if !line_bytes.is_power_of_two() {
            return Err(CacheFailure::LineSize);
        }
        let Some(mask) = line_bytes.checked_sub(1) else {
            return Err(CacheFailure::LineSize);
        };
        let Some(end) = start.checked_add(len) else {
            return Err(CacheFailure::RangeOverflow);
        };
        let Some(end) = end.checked_add(mask) else {
            return Err(CacheFailure::RangeOverflow);
        };
        let start = start & !mask;
        let Some(size) = (end & !mask).checked_sub(start) else {
            return Err(CacheFailure::RangeOverflow);
        };
        if size > max_bytes {
            return Err(CacheFailure::RangeTooLarge);
        }
        Ok(Self { start, size })
    }

    /// The first byte of the first line.
    pub const fn start(self) -> u32 {
        self.start
    }

    /// The bytes of the whole lines.
    pub const fn size(self) -> u32 {
        self.size
    }
}

/// Why a cache operation's range was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheFailure {
    EmptyRange,
    RangeOverflow,
    RangeTooLarge,
    LineSize,
}

impl CacheFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::EmptyRange => "cache-range-empty",
            Self::RangeOverflow => "cache-range-overflow",
            Self::RangeTooLarge => "cache-range-size",
            Self::LineSize => "cache-line-size",
        }
    }
}

/// Proof that a range was published to instruction fetch: its lines written
/// back and the instruction caches' copies dropped. The caller still
/// executes `fence.i` before fetching it.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the published range is entered only after `fence.i`"]
pub struct PublishedCode {
    range: LineRange,
}

impl PublishedCode {
    /// The whole lines published.
    pub const fn range(&self) -> LineRange {
        self.range
    }
}

/// Publish `start..start + len` to instruction fetch, each command waited
/// for with `wait` under `deadline`.
pub fn publish_code<R, C>(
    registers: &mut R,
    geometry: CacheGeometry<R::Map>,
    start: u32,
    len: u32,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<PublishedCode, BootFailure>
where
    R: CacheSyncRegisters,
    C: Clock + ?Sized,
{
    let range = LineRange::cover(start, len, geometry.line_bytes, geometry.max_range_bytes)?;
    registers.set_range(Access::new(), geometry.data, range);
    for _ in 0..geometry.writeback_passes {
        run(registers, SyncCommand::Writeback, wait, deadline)?;
    }
    if let Some(instruction) = geometry.instruction {
        registers.set_range(Access::new(), instruction, range);
        run(registers, SyncCommand::Invalidate, wait, deadline)?;
    }
    Ok(PublishedCode { range })
}

fn run<R, C>(
    registers: &mut R,
    command: SyncCommand,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure>
where
    R: CacheSyncRegisters,
    C: Clock + ?Sized,
{
    deadline.poll(wait, || registers.start(Access::new(), command).ok())?;
    deadline.poll(wait, || registers.done(Access::new()).then_some(()))?;
    Ok(())
}

/// Drop the data cache's lines of `start..start + len`, widened to whole
/// lines: memory the cache served stale (a range just mapped) is read
/// again from its device.
///
/// SOURCE(esp32s31): ESP-IDF 4d80c1b2
/// `components/esp_rom/esp32s31/include/esp32s31/rom/cache.h`
/// (`Cache_Invalidate_Addr`, drop cached data for a range; data map BIT(4))
/// and `components/soc/esp32s31/register/soc/cache_reg.h:1574-1640`
/// (`INVALIDATE_ENA`, `SYNC_DONE`, `SYNC_MAP`).
pub fn invalidate_data<R, C>(
    registers: &mut R,
    geometry: CacheGeometry<R::Map>,
    start: u32,
    len: u32,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<LineRange, BootFailure>
where
    R: CacheSyncRegisters,
    C: Clock + ?Sized,
{
    let range = LineRange::cover(start, len, geometry.line_bytes, geometry.max_range_bytes)?;
    registers.set_range(Access::new(), geometry.data, range);
    run(registers, SyncCommand::Invalidate, wait, deadline)?;
    Ok(range)
}

/// The words that suspend core 0's instruction fetch from the cache.
pub trait FetchSuspendRegisters {
    /// Shut (or open) core 0's instruction bus to the cache.
    fn shut_fetch(&mut self, access: Access<'_>, shut: bool);
    /// Whether instruction-cache autoload is enabled.
    fn autoload_enabled(&self, access: Access<'_>) -> bool;
    /// Enable or disable instruction-cache autoload.
    fn set_autoload(&mut self, access: Access<'_>, enabled: bool);
    /// Whether autoload is idle.
    fn autoload_done(&self, access: Access<'_>) -> bool;
    /// Whether the instruction cache's preload is idle.
    fn preload_done(&self, access: Access<'_>) -> bool;
    /// Whether the sync engine is idle.
    fn sync_done(&self, access: Access<'_>) -> bool;
    /// Whether the lock engine is idle.
    fn lock_done(&self, access: Access<'_>) -> bool;
    /// Select freeze mode 0, the mode the ROM's suspension freezes with.
    fn select_freeze_mode_zero(&mut self, access: Access<'_>);
    /// Freeze (or thaw) the instruction cache.
    fn set_freeze(&mut self, access: Access<'_>, frozen: bool);
    /// Whether the freeze has taken effect (true) or the thaw (false).
    fn freeze_done(&self, access: Access<'_>) -> bool;
}

/// The named waits of a fetch suspension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SuspendWaits {
    pub autoload: BootWait,
    pub preload: BootWait,
    pub sync: BootWait,
    pub lock: BootWait,
    pub freeze: BootWait,
    pub thaw: BootWait,
}

/// Run `operation` with core 0's instruction fetch from the cache
/// suspended, and resume it however the operation ends. The caller runs
/// from memory the cache does not serve (SRAM or ROM).
pub fn with_fetch_suspended<R, C, T>(
    registers: &mut R,
    waits: SuspendWaits,
    deadline: &BootDeadline<'_, C>,
    operation: impl FnOnce() -> T,
) -> Result<T, BootFailure>
where
    R: FetchSuspendRegisters,
    C: Clock + ?Sized,
{
    let suspended = suspend_fetch(registers, waits, deadline)?;
    let result = operation();
    resume_fetch(registers, suspended);
    Ok(result)
}

/// Instruction fetch suspended by [`suspend_fetch`]: whether autoload ran,
/// which [`resume_fetch`] restarts.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "instruction fetch stays suspended until `resume_fetch`"]
pub struct SuspendedFetch {
    autoload: bool,
}

/// Suspend core 0's instruction fetch from the cache; on a failed wait,
/// what the suspension changed is undone before it returns.
pub fn suspend_fetch<R, C>(
    registers: &mut R,
    waits: SuspendWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<SuspendedFetch, BootFailure>
where
    R: FetchSuspendRegisters,
    C: Clock + ?Sized,
{
    suspend(registers, waits, deadline).map(|autoload| SuspendedFetch { autoload })
}

/// Resume the instruction fetch `suspended` stopped.
pub fn resume_fetch<R: FetchSuspendRegisters>(registers: &mut R, suspended: SuspendedFetch) {
    resume(registers, suspended.autoload, false);
}

fn suspend<R, C>(
    registers: &mut R,
    waits: SuspendWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<bool, BootFailure>
where
    R: FetchSuspendRegisters,
    C: Clock + ?Sized,
{
    registers.shut_fetch(Access::new(), true);
    let autoload = registers.autoload_enabled(Access::new());
    if autoload {
        registers.set_autoload(Access::new(), false);
    }
    let idle = [
        (
            waits.autoload,
            R::autoload_done as fn(&R, Access<'_>) -> bool,
        ),
        (waits.preload, R::preload_done),
        (waits.sync, R::sync_done),
        (waits.lock, R::lock_done),
    ];
    for (wait, idle) in idle {
        if let Err(failure) = deadline.poll(wait, || idle(registers, Access::new()).then_some(())) {
            resume(registers, autoload, false);
            return Err(failure.into());
        }
    }
    registers.select_freeze_mode_zero(Access::new());
    registers.set_freeze(Access::new(), true);
    if let Err(failure) = deadline.poll(waits.freeze, || {
        registers.freeze_done(Access::new()).then_some(())
    }) {
        resume(registers, autoload, true);
        return Err(failure.into());
    }
    registers.set_freeze(Access::new(), false);
    if let Err(failure) = deadline.poll(waits.thaw, || {
        (!registers.freeze_done(Access::new())).then_some(())
    }) {
        resume(registers, autoload, false);
        return Err(failure.into());
    }
    Ok(autoload)
}

/// Undo a suspension: thaw if still frozen, open the instruction bus, then
/// restart autoload if it ran.
fn resume<R: FetchSuspendRegisters>(registers: &mut R, autoload: bool, frozen: bool) {
    if frozen {
        registers.set_freeze(Access::new(), false);
    }
    registers.shut_fetch(Access::new(), false);
    if autoload {
        registers.set_autoload(Access::new(), true);
    }
}
