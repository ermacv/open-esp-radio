//! The chip's two HP cores (harts): the cores' owner ([`Cores`]), the fatal
//! path's stop of every other hart ([`StopTheWorld`]) and the chip's software
//! reset ([`software_reset`]).
//!
//! The core-control words have one holder, an atomic state that only a
//! compare-and-swap changes and nothing waits for:
//!
//! - `Free → Owned` when the image takes the cores' owner on hart 0
//!   ([`Cores::take`]); each of its operations holds the words (`Owned →
//!   Busy → Owned`) for a few register accesses and a bounded wait.
//! - `Free | Owned → Fatal(hart)` when a fatal path of `hart` takes them, for
//!   good. That compare-and-swap is the election of two harts that fail at
//!   once: the loser never requests a stall, so two fatal harts never stall
//!   each other. A fatal path that finds the cores' owner in an operation
//!   does not wait for it: the owner may be the code it interrupted.
//!
//! A fatal path is the only one that gets a [`Fatal`]: the staged runtime's
//! fatal entries make it after masking the hart's interrupts, and the path
//! never returns to the code it interrupted ([`fatal`]). With it,
//! [`stop_the_world`] takes the words, stops the other hart and confirms it:
//! the hardware's stall status, or core 1 held in reset with its clock
//! gated. The [`StopTheWorld`] it returns lets its holder take over a block
//! that another owner holds ([`StopTheWorld::take_over`]), as no other
//! context runs again. A stall the hardware does not confirm within
//! [`STALL_POLLS`] is a typed failure: the caller keeps its record for the
//! next boot and resets.
//!
//! The stop confirms that the other hart executes nothing; it does not
//! prove that the bus transactions it issued completed, nor does it stop
//! DMA engines, the LP core or a debugger. A block's takeover resets the
//! state its owner may have left.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};

pub use oer_esp32s31_pac::platform::HpCore;
use oer_esp32s31_pac::platform::{CoreControlHandoff, CoreControlRegisters};
use oer_mmio::Placement;

#[cfg(test)]
mod tests;

#[cfg(feature = "cpu1-runs")]
compile_error!(
    "the runtime's critical-section masks only the current hart; an image whose hart 1 runs code needs a cross-hart lock without spinning"
);

/// Polls of the stall status that a stall or run request waits for its
/// confirmation. No source states the hardware's latency (ESP-IDF stalls
/// without waiting and waits without a bound for the run); a poll is one
/// register read, so the budget is milliseconds, far beyond a pipeline's
/// drain, and a hart that does not stop within it is reported, not waited
/// for.
pub const STALL_POLLS: u32 = 10_000;

/// Keeps a value on the hart that made it: not `Send`.
type ThisHart = PhantomData<*const ()>;

/// No one holds the words.
const FREE: u8 = 0;
/// The cores' owner exists and runs no operation.
const OWNED: u8 = 1;
/// The cores' owner runs one operation.
const BUSY: u8 = 2;
/// Hart 0's fatal path holds the words, for good.
const FATAL_0: u8 = 3;
/// Hart 1's fatal path holds the words, for good.
const FATAL_1: u8 = 4;

const fn fatal_state(hart: HpCore) -> u8 {
    match hart {
        HpCore::Core0 => FATAL_0,
        HpCore::Core1 => FATAL_1,
    }
}

/// The hart that is not `hart`.
const fn other(hart: HpCore) -> HpCore {
    match hart {
        HpCore::Core0 => HpCore::Core1,
        HpCore::Core1 => HpCore::Core0,
    }
}

/// Who holds the core-control words.
pub(crate) struct Holder(AtomicU8);

/// What a fatal path's claim of the words found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Election {
    /// The words are this fatal path's now.
    Won,
    /// This hart's fatal path took them before.
    Again,
    /// The other hart's fatal path took them.
    Lost,
    /// The cores' owner runs an operation.
    Busy,
}

impl Holder {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(FREE))
    }

    fn take_owner(&self) -> bool {
        self.0
            .compare_exchange(FREE, OWNED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn begin(&self) -> bool {
        self.0
            .compare_exchange(OWNED, BUSY, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn end(&self) {
        self.0.store(OWNED, Ordering::Release);
    }

    /// Claim the words for `hart`'s fatal path, for good. Never waits: one
    /// attempt from each state a claim moves from.
    fn elect(&self, hart: HpCore) -> Election {
        let ours = fatal_state(hart);
        let found = match self
            .0
            .compare_exchange(FREE, ours, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Election::Won,
            Err(OWNED) => {
                match self
                    .0
                    .compare_exchange(OWNED, ours, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => return Election::Won,
                    Err(found) => found,
                }
            }
            Err(found) => found,
        };
        if found == ours {
            Election::Again
        } else if found == FATAL_0 || found == FATAL_1 {
            Election::Lost
        } else {
            Election::Busy
        }
    }
}

/// The chip's one holder of the core-control words.
static HOLDER: Holder = Holder::new();

/// The words' owner, built for the holder's winner.
#[allow(
    unsafe_code,
    reason = "the cores' owner and the fatal path capture the words they hold"
)]
fn capture() -> CoreControlRegisters {
    // SAFETY: `HOLDER` calls for this only after its compare-and-swap made
    // the caller the words' one writer: the cores' owner for one operation,
    // which drops this owner before it frees the words, or a fatal path, for
    // good. A fatal path that took the words from the cores' owner on its
    // own hart never returns to the code it interrupted, and one that took
    // them on the other hart found no operation running. esp-hal writes
    // them only in esp_hal::init (`registers/shared-words.toml`).
    let handoff = unsafe { CoreControlHandoff::assume_exclusive() };
    CoreControlRegisters::from_handoff(handoff)
}

/// The hart this code runs on.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "reads mhartid")]
fn current_hart() -> HpCore {
    let hart: usize;
    // SAFETY: reading mhartid touches no memory and changes nothing.
    unsafe {
        core::arch::asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack, preserves_flags))
    };
    if hart == 0 {
        HpCore::Core0
    } else {
        HpCore::Core1
    }
}

/// The host runs the HAL's tests as hart 0.
#[cfg(not(target_arch = "riscv32"))]
fn current_hart() -> HpCore {
    HpCore::Core0
}

/// A stall or run request the hardware did not confirm within
/// [`STALL_POLLS`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unconfirmed;

/// Why the cores' owner did not run an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoresError {
    /// A fatal path holds the words for good.
    Stopped,
    /// The hardware did not confirm the request within [`STALL_POLLS`].
    Unconfirmed,
}

/// Wait for the hardware to report `hart` stalled (`stalled`) or running,
/// within `polls` reads.
fn confirm<P: Placement, Q: Placement, R: Placement, S: Placement>(
    words: &CoreControlRegisters<P, Q, R, S>,
    hart: HpCore,
    stalled: bool,
    polls: u32,
) -> Result<(), Unconfirmed> {
    oer_mmio::fence();
    for _ in 0..polls {
        if words.is_stalled(hart) == stalled {
            oer_mmio::fence();
            return Ok(());
        }
    }
    Err(Unconfirmed)
}

/// Request that `hart` stall and wait for the hardware to confirm it.
fn stall<P: Placement, Q: Placement, R: Placement, S: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S>,
    hart: HpCore,
    polls: u32,
) -> Result<(), Unconfirmed> {
    words.request_stall(hart);
    confirm(words, hart, true, polls)
}

/// Request that `hart` run and wait for the hardware to report it no longer
/// stalled.
fn run<P: Placement, Q: Placement, R: Placement, S: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S>,
    hart: HpCore,
    polls: u32,
) -> Result<(), Unconfirmed> {
    words.request_run(hart);
    confirm(words, hart, false, polls)
}

/// How the other hart was found stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stopped {
    /// Core 1 is held in its global reset with its CPU clock gated: it
    /// executes nothing, and no stall status is defined for it.
    Held,
    /// The hardware reports the hart stalled.
    Stalled,
}

/// Stop the hart that is not `this` and confirm it.
fn stop_other<P: Placement, Q: Placement, R: Placement, S: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S>,
    this: HpCore,
    polls: u32,
) -> Result<Stopped, Unconfirmed> {
    let other = other(this);
    if other == HpCore::Core1 && words.core1_held() {
        return Ok(Stopped::Held);
    }
    stall(words, other, polls).map(|()| Stopped::Stalled)
}

/// Where hart 1 starts after its reset: the ROM's boot address word.
trait BootAddress {
    /// Make hart 1's ROM jump to `entry`; zero keeps it waiting in the ROM.
    fn set(&mut self, entry: u32);
}

/// Prepare the chip's reset on `this` hart: UART0's clock for the boot ROM,
/// no boot address for hart 1, then the other hart reset and stalled. The
/// boot address is cleared before the reset pulse, so a running hart 1
/// that the pulse sends into the ROM finds no entry of this image.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/system.rs` (`software_reset`) and
/// `esp-hal/src/soc/esp32s31/cpu_control.rs` (`pre_system_reset`), which
/// clear the boot address after the stall; ESP-IDF 4d59230d
/// `components/esp_system/port/soc/esp32s31/system_internal.c`.
fn prepare_reset<P: Placement, Q: Placement, R: Placement, S: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S>,
    boot: &mut impl BootAddress,
    this: HpCore,
) {
    words.restore_uart0_clock();
    boot.set(0);
    let other = other(this);
    words.reset_core(other);
    words.request_stall(other);
}

/// The ROM routines this module calls.
mod rom {
    /// The ROM's boot address of hart 1.
    pub(super) struct Rom;

    #[cfg(target_arch = "riscv32")]
    unsafe extern "C" {
        /// SOURCE: the pinned ESP32-S31 rev0 ROM (`esp32s31_rev0_rom.elf`,
        /// `chips/esp32s31/vendor/artifacts.toml`): Blobray's function
        /// record of `ets_set_appcpu_boot_addr` stores its argument whole to
        /// `LP_SYS.BOOT_ADDR_HP_CORE1` (0x20700164) and returns.
        fn ets_set_appcpu_boot_addr(entry: u32);
        /// SOURCE: the same ROM: `software_reset` sets `LP_SYS.SYS_CTRL`'s
        /// `SYS_SW_RST` (0x20700008 bit 1), which resets the HP system.
        fn software_reset() -> !;
    }

    impl super::BootAddress for Rom {
        #[cfg(target_arch = "riscv32")]
        #[allow(unsafe_code, reason = "a ROM routine")]
        fn set(&mut self, entry: u32) {
            // SAFETY: the routine stores `entry` to the ROM's boot address
            // word, which nothing of the image reads, and touches no other
            // memory.
            unsafe { ets_set_appcpu_boot_addr(entry) };
        }

        /// The host has no ROM.
        #[cfg(not(target_arch = "riscv32"))]
        fn set(&mut self, _entry: u32) {}
    }

    /// Reset the HP system through the ROM.
    #[cfg(target_arch = "riscv32")]
    #[allow(unsafe_code, reason = "a ROM routine")]
    pub(super) fn system_reset() -> ! {
        // SAFETY: the routine requests the system reset and does not return.
        unsafe { software_reset() }
    }

    /// The host has no ROM.
    #[cfg(not(target_arch = "riscv32"))]
    #[expect(clippy::panic, reason = "no host code resets the chip")]
    pub(super) fn system_reset() -> ! {
        panic!("the system reset is the chip ROM's")
    }
}

/// The cores' owner: hart 1's start, park, stall and run. One exists per
/// image, on hart 0, which it never leaves (not `Send`).
pub struct Cores(ThisHart);

impl Cores {
    /// Take the cores' owner on hart 0, once: `None` on hart 1, after it was
    /// taken, or once a fatal path holds the words.
    pub fn take() -> Option<Self> {
        (current_hart() == HpCore::Core0 && HOLDER.take_owner()).then_some(Self(PhantomData))
    }

    fn operate<T>(
        &mut self,
        operation: impl FnOnce(&mut CoreControlRegisters) -> T,
    ) -> Result<T, CoresError> {
        if !HOLDER.begin() {
            return Err(CoresError::Stopped);
        }
        let result = operation(&mut capture());
        HOLDER.end();
        Ok(result)
    }

    /// How hart 1 stands: held in reset with its clock gated, stalled by the
    /// hardware's report, or neither.
    ///
    /// # Errors
    /// A fatal path holds the words ([`CoresError::Stopped`]).
    pub fn hart1(&mut self) -> Result<Hart1, CoresError> {
        self.operate(|words| {
            if words.core1_held() {
                Hart1::Held
            } else if words.is_stalled(HpCore::Core1) {
                Hart1::Stalled
            } else {
                Hart1::Released
            }
        })
    }

    /// Park hart 1 as ESP-IDF's single-core start does: request its stall,
    /// then gate its clocks and hold it in its global reset.
    ///
    /// # Errors
    /// A fatal path holds the words ([`CoresError::Stopped`]).
    pub fn park_hart1(&mut self) -> Result<(), CoresError> {
        self.operate(|words| {
            words.request_stall(HpCore::Core1);
            words.hold_core1();
        })
    }

    /// Stall hart 1 and wait for the hardware to confirm it.
    ///
    /// # Errors
    /// A fatal path holds the words, or the hardware did not confirm the
    /// stall within [`STALL_POLLS`].
    pub fn stall_hart1(&mut self) -> Result<(), CoresError> {
        self.operate(|words| stall(words, HpCore::Core1, STALL_POLLS))?
            .map_err(|Unconfirmed| CoresError::Unconfirmed)
    }

    /// Let hart 1 run and wait for the hardware to report it no longer
    /// stalled.
    ///
    /// # Errors
    /// A fatal path holds the words, or the hardware did not confirm the run
    /// within [`STALL_POLLS`].
    pub fn run_hart1(&mut self) -> Result<(), CoresError> {
        self.operate(|words| run(words, HpCore::Core1, STALL_POLLS))?
            .map_err(|Unconfirmed| CoresError::Unconfirmed)
    }

    /// Start hart 1 at `entry`, as ESP-IDF starts the other core: no boot
    /// address, the run request confirmed, the clocks enabled and the
    /// global reset released, then the entry.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/esp_system/port/cpu_start.c`
    /// (`start_other_core`).
    ///
    /// # Errors
    /// A fatal path holds the words, or the hardware did not confirm the run
    /// within [`STALL_POLLS`].
    #[cfg(feature = "cpu1-runs")]
    pub fn start_hart1(&mut self, entry: extern "C" fn() -> !) -> Result<(), CoresError> {
        self.operate(|words| {
            let mut boot = rom::Rom;
            boot.set(0);
            run(words, HpCore::Core1, STALL_POLLS)?;
            words.release_core1();
            boot.set(entry as usize as u32);
            Ok(())
        })?
        .map_err(|Unconfirmed| CoresError::Unconfirmed)
    }
}

/// How hart 1 stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hart1 {
    /// Held in its global reset with its CPU clock gated.
    Held,
    /// The hardware reports it stalled.
    Stalled,
    /// Neither held nor stalled: it may run code.
    Released,
}

/// Proof that this hart runs a fatal path: its interrupts masked, it never
/// returns to the code it interrupted, and it stays on this hart (not
/// `Send`). Only [`fatal`] makes one, and the proof does not leave the
/// closure it is lent to.
pub struct Fatal<'a> {
    hart: HpCore,
    _path: PhantomData<fn(&'a ()) -> &'a ()>,
    _hart: ThisHart,
}

impl Fatal<'_> {
    /// The hart that fails.
    pub const fn hart(&self) -> HpCore {
        self.hart
    }
}

/// Lend `fatal_path` the proof that this hart runs a fatal path.
///
/// # Safety
/// Call only from a fatal path of the current hart that masked the hart's
/// interrupts (the CLIC threshold at its maximum, then `mstatus.MIE` clear)
/// and that, after `fatal_path` returns, halts the hart or resets the chip
/// without unmasking them or returning to the code it interrupted.
#[allow(unsafe_code, reason = "the fatal-path contract of the token")]
pub unsafe fn fatal<T>(fatal_path: impl for<'a> FnOnce(Fatal<'a>) -> T) -> T {
    fatal_path(Fatal {
        hart: current_hart(),
        _path: PhantomData,
        _hart: PhantomData,
    })
}

/// Why a fatal path did not stop the world.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopError {
    /// The other hart's fatal path holds the core-control words: it stops
    /// this hart, which takes over nothing.
    Lost,
    /// The cores' owner runs an operation, which this path may have
    /// interrupted; it is not waited for.
    Busy,
    /// This hart's fatal path took the words before: a fatal path inside a
    /// fatal path takes over nothing.
    Reentered,
    /// The hardware did not confirm the other hart's stall within
    /// [`STALL_POLLS`].
    Unconfirmed,
}

/// Proof that no context but this fatal path runs on the chip's HP harts:
/// this hart's interrupts are masked and the other hart is confirmed
/// stopped. It stays in its fatal path, on its hart (not `Send`).
pub struct StopTheWorld<'a> {
    hart: HpCore,
    stopped: Stopped,
    _path: PhantomData<fn(&'a ()) -> &'a ()>,
    _hart: ThisHart,
}

impl StopTheWorld<'_> {
    /// The hart of the fatal path.
    pub const fn hart(&self) -> HpCore {
        self.hart
    }

    /// How the other hart was found stopped.
    pub const fn other(&self) -> Stopped {
        self.stopped
    }

    /// Take over a block another owner holds: that owner never runs again.
    /// The block's [`TakeOver`] resets what its owner left and refuses a
    /// second takeover while the first one's owner may live.
    pub fn take_over<T: TakeOver>(&mut self) -> Option<T> {
        T::take_over(self)
    }
}

/// A block owner a fatal path takes over once the world stopped.
pub trait TakeOver: Sized + sealed::Sealed {
    /// The block's owner, its state reset; `None` once taken over before.
    fn take_over(stop: &mut StopTheWorld<'_>) -> Option<Self>;
}

pub(crate) mod sealed {
    /// Only this HAL's blocks are taken over.
    pub trait Sealed {}
}

/// Stop the world from `fatal`'s path: claim the core-control words for
/// good, then stop the other hart and confirm it.
///
/// # Errors
/// [`StopError`]: the other hart's fatal path or the cores' owner holds the
/// words, this hart stopped the world before, or the other hart's stall was
/// not confirmed. Nothing is taken over then; the caller keeps its record
/// and resets.
pub fn stop_the_world(fatal: Fatal<'_>) -> Result<StopTheWorld<'_>, StopError> {
    let hart = fatal.hart;
    let stopped = stop_with(&HOLDER, capture, hart, STALL_POLLS)?;
    Ok(StopTheWorld {
        hart,
        stopped,
        _path: PhantomData,
        _hart: PhantomData,
    })
}

fn stop_with<P: Placement, Q: Placement, R: Placement, S: Placement>(
    holder: &Holder,
    capture: impl FnOnce() -> CoreControlRegisters<P, Q, R, S>,
    hart: HpCore,
    polls: u32,
) -> Result<Stopped, StopError> {
    match holder.elect(hart) {
        Election::Won => {}
        Election::Again => return Err(StopError::Reentered),
        Election::Lost => return Err(StopError::Lost),
        Election::Busy => return Err(StopError::Busy),
    }
    stop_other(&mut capture(), hart, polls).map_err(|Unconfirmed| StopError::Unconfirmed)
}

/// Reset the chip from `fatal`'s path, as the boot ROM expects it: UART0's
/// clock restored, no boot address for hart 1, the other hart reset and
/// stalled, then the ROM's system reset. A path that cannot hold the
/// core-control words (the other hart's fatal path, or the cores' owner in
/// an operation) resets through the ROM alone.
pub fn software_reset(fatal: Fatal<'_>) -> ! {
    reset_with(&HOLDER, capture, &mut rom::Rom, fatal.hart);
    oer_mmio::fence();
    rom::system_reset()
}

/// Whether the reset was prepared, holding the words.
fn reset_with<P: Placement, Q: Placement, R: Placement, S: Placement>(
    holder: &Holder,
    capture: impl FnOnce() -> CoreControlRegisters<P, Q, R, S>,
    boot: &mut impl BootAddress,
    hart: HpCore,
) -> bool {
    match holder.elect(hart) {
        Election::Won | Election::Again => {
            prepare_reset(&mut capture(), boot, hart);
            true
        }
        Election::Lost | Election::Busy => false,
    }
}
