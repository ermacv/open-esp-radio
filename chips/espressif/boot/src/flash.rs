//! The boot's flash: a read through the SPI1 user command port, and the
//! flash clock's timing sweep.
//!
//! **Read.** The boot reads flash itself, with the read command the ROM's
//! `esp_rom_spiflash_read` picks for the controller's read mode (the mode
//! the bootloader configured): the user command, address and dummy phases
//! of that command, once no command of the command word runs, then for
//! each chunk of at most [`CHUNK_WORDS`] words
//! the address and the input length, the user command started once none
//! runs, its completion waited for, and the data words copied out. A read
//! outside the flash, or one whose address is not word aligned, is refused
//! before any register is written. The boot programs and erases nothing.
//!
//! SOURCE(esp32s31): pinned rev0 ROM (esp-rom-elfs 20260528, SHA-256
//! a52ad751...ab87), Blobray function records of `_esp_rom_spiflash_read`
//! @0x2f80f0c2 (the read mode decoded from `CTRL`, `USER` and `USER2` set),
//! `esp_rom_spi_set_rd_cmd_bit_len` @0x2f80e020 (`USER1`, the address and
//! dummy phases) and `SPI_read_data` @0x2f80e24a (`ADDR`, `MISO_DLEN`,
//! `CMD.USR`, the data words); ESP-IDF 4d59230d
//! `components/esp_hal_mspi/spi_flash_hal_common.inc:239-252` (one read:
//! address, input length, start, completion, data) and
//! `components/esp_hal_mspi/esp32s31/include/hal/spimem_flash_ll.h:394-412`
//! (the data words).
//!
//! **Timing sweep.** The flash clock moves to the sweep's rate, and each
//! candidate timing must read back what the boot read at the bootloader's
//! rate: first a direct 128-byte prefix through the command port, then a
//! whole untouched XIP page through the cache. The candidate in the middle
//! of the longest run of at least three that pass both (the first on a
//! tie) is applied; the remaining pages must then read back through the
//! cache. Any failure restores every timing word and the clock under a
//! suspended fetch, then fails the boot: nothing falls back to the
//! bootloader's rate. Where fetch cannot be suspended again, nothing is
//! restored and the boot fails as `RollbackUnsuspended`. Each XIP page is
//! read once, cold, because a rejected candidate can leave corrupted lines
//! in the cache.
//!
//! SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:183-383` (the
//! sweep) and `esp-hal/src/flash/tuning.rs:6-53` (`page_count`,
//! `select_window`, ported here).

use oer_time::Clock;

use crate::{
    Access, BootFailure, Busy,
    cache::{FetchSuspendRegisters, SuspendWaits, resume_fetch, suspend_fetch},
    wait::{BootDeadline, BootWait},
};

/// The data words one user command reads at most: 64 bytes.
///
/// SOURCE: ESP-IDF 4d59230d `components/esp_hal_mspi/include/hal/spi_flash_hal.h:28`
/// (`SPI_FLASH_HAL_MAX_READ_BYTES`) and
/// `components/soc/esp32s31/register/soc/spi1_mem_c_struct.h:1080` (`data_buf[16]`).
pub const CHUNK_WORDS: usize = 16;

/// The flash controller's read mode, as the bootloader configured it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlashReadMode {
    Qio,
    Dio,
    Qout,
    Dout,
    FastRead,
    SlowRead,
}

/// One read command: its opcode, the bits of its address phase (the
/// address and any continuous-mode bits sent with it) and its dummy cycles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashReadCommand {
    pub opcode: u8,
    pub address_bits: u8,
    pub dummy_cycles: u8,
}

/// The read command of every read mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashReadCommands {
    pub qio: FlashReadCommand,
    pub dio: FlashReadCommand,
    pub qout: FlashReadCommand,
    pub dout: FlashReadCommand,
    pub fast_read: FlashReadCommand,
    pub slow_read: FlashReadCommand,
}

impl FlashReadCommands {
    /// The command of `mode`.
    pub const fn get(&self, mode: FlashReadMode) -> FlashReadCommand {
        match mode {
            FlashReadMode::Qio => self.qio,
            FlashReadMode::Dio => self.dio,
            FlashReadMode::Qout => self.qout,
            FlashReadMode::Dout => self.dout,
            FlashReadMode::FastRead => self.fast_read,
            FlashReadMode::SlowRead => self.slow_read,
        }
    }
}

/// What the boot knows of its flash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashGeometry {
    /// The flash's bytes.
    pub capacity: u32,
    /// The read command of each read mode.
    pub commands: FlashReadCommands,
}

/// The SPI1 user command port's words a read uses.
pub trait FlashCommandRegisters {
    /// The read mode the controller is configured for.
    fn read_mode(&self, access: Access<'_>) -> FlashReadMode;
    /// Whether the controller sends continuous-mode bits from its status
    /// word; the boot's read sends them in the address phase instead.
    fn mode_bits_from_status(&self, access: Access<'_>) -> bool;
    /// Set the command, address and dummy phases of `command`, with an
    /// input phase and no output phase.
    fn configure_read(&mut self, access: Access<'_>, command: FlashReadCommand);
    /// Set the next command's address and input length in bytes.
    fn set_transfer(&mut self, access: Access<'_>, address: u32, bytes: u32);
    /// Start the user command, or [`Busy`] while one runs.
    fn start(&mut self, access: Access<'_>) -> Result<(), Busy>;
    /// Whether any command of the command word still runs: the user
    /// command or any other the word starts.
    fn busy(&self, access: Access<'_>) -> bool;
    /// The data words of the last command.
    fn data(&self, access: Access<'_>) -> [u32; CHUNK_WORDS];
}

/// Why a flash read or the flash timing was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlashFailure {
    /// The read's address is not word aligned.
    Misaligned,
    /// The read reaches past the flash.
    OutsideFlash,
    /// The controller sends continuous-mode bits from its status word.
    StatusModeBits,
    /// The tuning region is misaligned, too small or outside the flash, or
    /// the candidates are none or more than sixteen.
    TuningRegion,
    /// The scratch is shorter than the tuning region's pages.
    ScratchTooSmall,
    /// No run of three candidates passed both reads.
    NoStableWindow,
    /// A page read back otherwise through the cache at the chosen timing.
    CacheMismatch,
    /// The sweep failed and fetch could not be suspended again to roll it
    /// back: the clock and the timing stay at the sweep's.
    RollbackUnsuspended,
}

impl FlashFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Misaligned => "flash-misaligned",
            Self::OutsideFlash => "flash-outside",
            Self::StatusModeBits => "flash-status-mode-bits",
            Self::TuningRegion => "flash-tuning-region",
            Self::ScratchTooSmall => "flash-tuning-scratch",
            Self::NoStableWindow => "flash-no-stable-window",
            Self::CacheMismatch => "flash-cache-mismatch",
            Self::RollbackUnsuspended => "flash-rollback-unsuspended",
        }
    }
}

/// Read `destination.len()` words of flash from `address`, each user
/// command waited for with `wait` under `deadline`.
pub fn read<R, C>(
    registers: &mut R,
    flash: FlashGeometry,
    address: u32,
    destination: &mut [u32],
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure>
where
    R: FlashCommandRegisters,
    C: Clock + ?Sized,
{
    if !address.is_multiple_of(4) {
        return Err(FlashFailure::Misaligned.into());
    }
    let bytes = u32::try_from(destination.len())
        .ok()
        .and_then(|words| words.checked_mul(4))
        .ok_or(FlashFailure::OutsideFlash)?;
    if address
        .checked_add(bytes)
        .is_none_or(|end| end > flash.capacity)
    {
        return Err(FlashFailure::OutsideFlash.into());
    }
    if registers.mode_bits_from_status(Access::new()) {
        return Err(FlashFailure::StatusModeBits.into());
    }
    let mode = registers.read_mode(Access::new());
    // No phase of a running command changes: the whole command word is
    // idle before the read configures it.
    deadline.poll(wait, || (!registers.busy(Access::new())).then_some(()))?;
    registers.configure_read(Access::new(), flash.commands.get(mode));
    let mut chunk_address = address;
    for chunk in destination.chunks_mut(CHUNK_WORDS) {
        // A chunk holds at most sixteen words: its bytes fit.
        let chunk_bytes = (chunk.len() * 4) as u32;
        registers.set_transfer(Access::new(), chunk_address, chunk_bytes);
        deadline.poll(wait, || registers.start(Access::new()).ok())?;
        deadline.poll(wait, || (!registers.busy(Access::new())).then_some(()))?;
        let data = registers.data(Access::new());
        for (word, &read) in chunk.iter_mut().zip(&data) {
            *word = read;
        }
        chunk_address += chunk_bytes;
    }
    Ok(())
}

/// The flash clock's words the sweep moves and restores.
pub trait FlashClockRegisters {
    /// The words as they read before the sweep.
    type Saved: Copy;
    /// Read the words.
    fn save_clock(&self, access: Access<'_>) -> Self::Saved;
    /// Move the flash clock to the sweep's rate.
    fn select_tuning_clock(&mut self, access: Access<'_>);
    /// Write the words back.
    fn restore_clock(&mut self, access: Access<'_>, saved: Self::Saved);
}

/// The controllers' input timing words.
pub trait FlashTimingRegisters {
    /// One candidate timing.
    type Timing: Copy;
    /// The words as they read before the sweep.
    type Saved: Copy;
    /// Read the words.
    fn save_timing(&self, access: Access<'_>) -> Self::Saved;
    /// Move the controllers to the sweep's clock (dummy input on the
    /// controller's clock).
    fn select_tuning_input(&mut self, access: Access<'_>);
    /// Apply `timing` to the command port, and to the cache's reads too
    /// when `cache` is set, and latch it.
    fn apply_timing(&mut self, access: Access<'_>, timing: Self::Timing, cache: bool);
    /// Write the words back and latch them.
    fn restore_timing(&mut self, access: Access<'_>, saved: Self::Saved);
}

/// The tuning region's mapping, read through the cache.
pub trait XipWindow {
    /// Word `index` of the region's mapping.
    fn word(&self, index: usize) -> u32;
}

/// The flash pages the sweep compares, mapped for instruction fetch at
/// `virtual_start`; nothing else reads them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TuningRegion {
    pub physical_start: u32,
    pub virtual_start: u32,
    pub bytes: u32,
}

/// A page of the sweep.
pub const TUNING_PAGE_BYTES: u32 = 4096;
const TUNING_PAGE_WORDS: usize = TUNING_PAGE_BYTES as usize / 4;
/// The most pages the sweep compares.
pub const TUNING_MAX_PAGES: usize = 31;
/// The scratch words the sweep needs for its most pages.
pub const TUNING_SCRATCH_WORDS: usize = TUNING_MAX_PAGES * TUNING_PAGE_WORDS;
/// The direct reference's words: a 128-byte prefix at flash address 0.
const PREFIX_WORDS: usize = 32;

/// The sweep's result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashTiming {
    /// The applied candidate.
    pub index: u8,
    /// One bit per candidate that passed the direct read.
    pub direct_mask: u16,
    /// One bit per candidate that also passed the cached read.
    pub cache_mask: u16,
}

/// The named waits of the sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TuningWaits {
    /// One user command.
    pub command: BootWait,
    /// The fetch suspension around the direct reads.
    pub suspend: SuspendWaits,
}

/// The pages of `region` the sweep compares: whole 4-KiB pages, at most
/// [`TUNING_MAX_PAGES`], at least one per candidate and three more, inside
/// the flash and the address space.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/tuning.rs:6-23`.
pub const fn page_count(region: TuningRegion, capacity: u32, candidates: usize) -> Option<usize> {
    if !region.physical_start.is_multiple_of(TUNING_PAGE_BYTES)
        || !region.virtual_start.is_multiple_of(TUNING_PAGE_BYTES)
    {
        return None;
    }
    let mut pages = (region.bytes / TUNING_PAGE_BYTES) as usize;
    if pages > TUNING_MAX_PAGES {
        pages = TUNING_MAX_PAGES;
    }
    let bytes = pages as u32 * TUNING_PAGE_BYTES;
    if pages < candidates + 3 {
        return None;
    }
    match region.physical_start.checked_add(bytes) {
        Some(end) if end <= capacity => {}
        _ => return None,
    }
    if region.virtual_start.checked_add(bytes).is_none() {
        return None;
    }
    Some(pages)
}

/// The middle of the longest run of at least three set bits among the low
/// `candidates` bits of `mask`; a tie keeps the first run.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/tuning.rs:25-53`.
pub const fn select_window(mask: u16, candidates: usize) -> Option<usize> {
    let mut longest_start = 0;
    let mut longest_len = 0;
    let mut current_start = 0;
    let mut current_len = 0;
    let mut candidate = 0;
    while candidate < candidates && candidate < 16 {
        if mask & (1 << candidate) != 0 {
            if current_len == 0 {
                current_start = candidate;
            }
            current_len += 1;
            if current_len > longest_len {
                longest_start = current_start;
                longest_len = current_len;
            }
        } else {
            current_len = 0;
        }
        candidate += 1;
    }
    if longest_len >= 3 {
        Some(longest_start + longest_len / 2)
    } else {
        None
    }
}

/// The owners the sweep moves: the command port and timing words, the
/// flash clock and the instruction-fetch suspension, and the tuning
/// region's mapping it reads.
pub struct TuningOwners<'a, F, K, S, X> {
    pub flash: &'a mut F,
    pub clock: &'a mut K,
    pub cache: &'a mut S,
    pub xip: &'a X,
}

/// Sweep `timings` at the flash clock's tuning rate and apply the chosen
/// one, comparing against `region` read into `scratch` first.
///
/// The caller runs from internal SRAM with interrupts masked and the
/// branch predictor off: instruction fetch from flash is suspended while
/// the timing changes.
pub fn tune<F, K, S, X, C>(
    owners: TuningOwners<'_, F, K, S, X>,
    flash: FlashGeometry,
    region: TuningRegion,
    timings: &[F::Timing],
    scratch: &mut [u32],
    waits: TuningWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<FlashTiming, BootFailure>
where
    F: FlashCommandRegisters + FlashTimingRegisters,
    K: FlashClockRegisters,
    S: FetchSuspendRegisters,
    X: XipWindow,
    C: Clock + ?Sized,
{
    let TuningOwners {
        flash: port,
        clock,
        cache,
        xip,
    } = owners;
    let candidates = timings.len();
    if candidates == 0 || candidates > 16 {
        return Err(FlashFailure::TuningRegion.into());
    }
    let pages = page_count(region, flash.capacity, candidates).ok_or(FlashFailure::TuningRegion)?;
    let Some(scratch) = scratch.get_mut(..pages * TUNING_PAGE_WORDS) else {
        return Err(FlashFailure::ScratchTooSmall.into());
    };
    let mut prefix = [0_u32; PREFIX_WORDS];
    read(port, flash, 0, &mut prefix, waits.command, deadline)?;
    for (page, words) in scratch.chunks_mut(TUNING_PAGE_WORDS).enumerate() {
        let address = region.physical_start + page as u32 * TUNING_PAGE_BYTES;
        read(port, flash, address, words, waits.command, deadline)?;
    }

    let saved_clock = clock.save_clock(Access::new());
    let saved_timing = port.save_timing(Access::new());
    let mut suspended = Some(suspend_fetch(cache, waits.suspend, deadline)?);
    clock.select_tuning_clock(Access::new());
    port.select_tuning_input(Access::new());

    let swept = sweep(
        port,
        cache,
        xip,
        flash,
        timings,
        &prefix,
        scratch,
        pages,
        &mut suspended,
        waits,
        deadline,
    );
    // The clock and the timing words change only while fetch is
    // suspended: where the suspension cannot be had again (a failed one
    // undid itself and left fetch running), they stay at the sweep's and
    // the boot fails with `RollbackUnsuspended`.
    let rollback = |port: &mut F,
                    clock: &mut K,
                    cache: &mut S,
                    suspended: Option<_>|
     -> Result<(), BootFailure> {
        let suspended = match suspended {
            Some(held) => held,
            None => suspend_fetch(cache, waits.suspend, deadline)
                .map_err(|_| FlashFailure::RollbackUnsuspended)?,
        };
        clock.restore_clock(Access::new(), saved_clock);
        port.restore_timing(Access::new(), saved_timing);
        resume_fetch(cache, suspended);
        Ok(())
    };
    let timing = match swept {
        Ok(timing) => timing,
        Err(failure) => {
            rollback(port, clock, cache, suspended.take())?;
            return Err(failure);
        }
    };
    if let Some(suspended) = suspended.take() {
        resume_fetch(cache, suspended);
    }
    // The pages no candidate read, through the cache at the chosen timing.
    let verified = (0..pages - candidates).all(|page| page_matches(xip, scratch, page));
    if !verified {
        rollback(port, clock, cache, None)?;
        return Err(FlashFailure::CacheMismatch.into());
    }
    Ok(timing)
}

/// The candidates' reads, fetch suspended on entry; on return `suspended`
/// holds the suspension if one is in force.
#[allow(
    clippy::too_many_arguments,
    reason = "one sweep over the owners, the references and its waits"
)]
fn sweep<F, S, X, C>(
    port: &mut F,
    cache: &mut S,
    xip: &X,
    flash: FlashGeometry,
    timings: &[F::Timing],
    prefix: &[u32; PREFIX_WORDS],
    scratch: &[u32],
    pages: usize,
    suspended: &mut Option<crate::cache::SuspendedFetch>,
    waits: TuningWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<FlashTiming, BootFailure>
where
    F: FlashCommandRegisters + FlashTimingRegisters,
    S: FetchSuspendRegisters,
    X: XipWindow,
    C: Clock + ?Sized,
{
    let mut direct_mask = 0_u16;
    for (candidate, &timing) in timings.iter().enumerate() {
        port.apply_timing(Access::new(), timing, false);
        let mut received = [0_u32; PREFIX_WORDS];
        read(port, flash, 0, &mut received, waits.command, deadline)?;
        if received == *prefix {
            direct_mask |= 1 << candidate;
        }
    }
    // Each candidate reads its own untouched page, so no cache hit passes
    // it falsely.
    let mut cache_mask = 0_u16;
    for (candidate, &timing) in timings.iter().enumerate() {
        if direct_mask & (1 << candidate) == 0 {
            continue;
        }
        port.apply_timing(Access::new(), timing, true);
        if let Some(held) = suspended.take() {
            resume_fetch(cache, held);
        }
        if page_matches(xip, scratch, pages - timings.len() + candidate) {
            cache_mask |= 1 << candidate;
        }
        *suspended = Some(suspend_fetch(cache, waits.suspend, deadline)?);
    }
    let usable = direct_mask & cache_mask;
    let best = select_window(usable, timings.len()).ok_or(FlashFailure::NoStableWindow)?;
    let chosen = timings
        .get(best)
        .copied()
        .ok_or(FlashFailure::NoStableWindow)?;
    port.apply_timing(Access::new(), chosen, true);
    Ok(FlashTiming {
        // At most sixteen candidates.
        index: best as u8,
        direct_mask,
        cache_mask,
    })
}

fn page_matches<X: XipWindow>(xip: &X, scratch: &[u32], page: usize) -> bool {
    let start = page * TUNING_PAGE_WORDS;
    scratch
        .get(start..start + TUNING_PAGE_WORDS)
        .is_some_and(|words| {
            words
                .iter()
                .enumerate()
                .all(|(offset, &word)| xip.word(start + offset) == word)
        })
}
