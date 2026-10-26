//! The chip's two HP cores (harts): the cores' owner ([`Cores`]), the fatal
//! path's stop of every other hart ([`StopTheWorld`]) and the chip's software
//! reset ([`software_reset`]).
//!
//! The reset root installs the core-control words once ([`install`]) in one
//! [`ClaimCell`], whose state only a compare-and-swap changes and nothing
//! waits for:
//!
//! - The image takes the cores' owner once, on hart 0 ([`Cores::take`]);
//!   each of its operations borrows the words (`Free → Lent → Free`) for a
//!   few register accesses and a bounded wait.
//! - `Free → Held(hart)` when a fatal path of `hart` takes them, for good;
//!   the same hart's path reaches them again, one call at a time. That
//!   compare-and-swap is the election of two harts that fail at once: the
//!   loser never requests a stall, so two fatal harts never stall each
//!   other. A fatal path that finds the words lent (an operation of the
//!   cores' owner, or its own hart's path it interrupted) does not wait
//!   for them.
//!
//! A fatal path is the only one that gets a [`Fatal`]: the staged runtime's
//! fatal entries make it after masking the hart's interrupts, and the path
//! never returns to the code it interrupted ([`fatal`]). With it,
//! [`stop_the_world`] takes the words, stops the other hart and confirms it:
//! the hardware's stall status, or core 1 held in reset with its clock
//! gated. A stall the hardware does not confirm within [`STALL_POLLS`] is a
//! typed failure. The stop only prepares the chip's reset: the fatal path
//! keeps its record for the next boot and resets ([`software_reset`]).
//!
//! The stop gives no right to a block another owner holds. The stall status
//! says that the other hart issues no new instruction; no source states
//! that the bus transactions it issued before completed (ESP-IDF's
//! `hp_system_reg.h` defines only the status, and the ESP32-S31 TRM has no
//! HP-CPU chapter), so a write of the stopped hart may still reach a block
//! after the stop. Nor does the stop halt DMA engines, the LP core or a
//! debugger. A takeover of a held block needs a documented drain after the
//! stall, or one a stand run shows, and a revocation of the old owner's
//! accesses.
//!
//! UART0's function clock, which the boot ROM prints through, has its own
//! cell: the reset restores it even when it cannot hold the core-control
//! words.
//!
//! A fatal path that cannot hold the core-control words resets through the
//! watchdog: [`software_reset`] returns [`AwaitWatchdog`] after the console's
//! fatal note (if the block is free), and the path halts its masked hart
//! until the RTC watchdog, which stage two arms when it takes its watchdogs
//! ([`crate::watchdog::Watchdogs`]), resets the chip; no second owner of the
//! words is made.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

use oer_diagnostics::Note;
use oer_memory::{Claim, ClaimCell, Held};
use oer_mmio::Placement;

pub use crate::platform::HpCore;
use crate::platform::core_control::{CoreControlRegisters, Uart0ClockRegisters};

#[cfg(test)]
mod tests;

#[cfg(feature = "cpu1-runs")]
compile_error!(
    "the runtime's critical-section masks only the current hart; an image whose hart 1 runs code needs a cross-hart lock without spinning"
);

/// Reads of the stall status that a stall or run request waits for its
/// confirmation; a hart the status does not confirm within them is
/// reported, not waited for. The budget counts completed reads, not time:
/// no source states the hardware's latency (ESP-IDF stalls without waiting
/// and waits without a bound for the run), and a read that never completes
/// is not bounded by it.
pub const STALL_POLLS: u32 = 10_000;

/// Keeps a value on the hart that made it: not `Send`.
type ThisHart = PhantomData<*const ()>;

/// A fatal path's key of its hart in the cells.
pub(crate) const fn key(hart: HpCore) -> u8 {
    match hart {
        HpCore::Core0 => 0,
        HpCore::Core1 => 1,
    }
}

/// The hart that is not `hart`.
const fn other(hart: HpCore) -> HpCore {
    match hart {
        HpCore::Core0 => HpCore::Core1,
        HpCore::Core1 => HpCore::Core0,
    }
}

/// The chip's core-control words, installed once by the reset root.
static CORE_CONTROL: ClaimCell<CoreControlRegisters> = ClaimCell::new();

/// UART0's function clock word, installed once by the reset root.
static UART0_CLOCK: ClaimCell<Uart0ClockRegisters> = ClaimCell::new();

/// Whether the cores' owner was taken.
static OWNER: AtomicBool = AtomicBool::new(false);

/// The core-control parts were installed before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installed;

/// Install the core-control words and UART0's clock word for the cores'
/// owner and the fatal paths, once, from the reset root.
pub(crate) fn install(
    core_control: CoreControlRegisters,
    uart0_clock: Uart0ClockRegisters,
) -> Result<(), Installed> {
    let core = CORE_CONTROL.install(core_control).map_err(|_| Installed);
    let uart0 = UART0_CLOCK.install(uart0_clock).map_err(|_| Installed);
    core.and(uart0)
}

/// The hart this code runs on.
#[cfg(target_arch = "riscv32")]
fn current_hart() -> HpCore {
    let hart = oer_riscv::hart_id();
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
    /// The reset root installed no core-control words.
    NotInstalled,
    /// A fatal path holds the words for good.
    Stopped,
    /// The hardware did not confirm the request within [`STALL_POLLS`].
    Unconfirmed,
}

/// Wait for the hardware to report `hart` stalled (`stalled`) or running,
/// within `polls` reads.
fn confirm<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &CoreControlRegisters<P, Q, R, S, T>,
    hart: HpCore,
    stalled: bool,
    polls: u32,
) -> Result<(), Unconfirmed> {
    oer_riscv::fence();
    for _ in 0..polls {
        if words.is_stalled(hart) == stalled {
            oer_riscv::fence();
            return Ok(());
        }
    }
    Err(Unconfirmed)
}

/// Request that `hart` stall and wait for the hardware to confirm it.
fn stall<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S, T>,
    hart: HpCore,
    polls: u32,
) -> Result<(), Unconfirmed> {
    words.request_stall(hart);
    confirm(words, hart, true, polls)
}

/// Request that `hart` run and wait for the hardware to report it no longer
/// stalled.
fn run<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S, T>,
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
fn stop_other<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S, T>,
    this: HpCore,
    polls: u32,
) -> Result<Stopped, Unconfirmed> {
    let other = other(this);
    if other == HpCore::Core1 && words.core1_held() {
        return Ok(Stopped::Held);
    }
    stall(words, other, polls).map(|()| Stopped::Stalled)
}

/// Prepare the chip's reset on `this` hart, holding the core-control words:
/// no boot address for hart 1, then the other hart reset and stalled. The
/// boot address is cleared before the reset pulse, so a running hart 1
/// that the pulse sends into the ROM finds no entry of this image.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_system/port/soc/esp32s31/system_internal.c:61-100`
/// (`esp_restart_noos_inner`: clear the APP CPU boot address before
/// resetting the cores, with the APP CPU stalled on the core-0 path).
fn prepare_reset<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S, T>,
    this: HpCore,
) {
    words.set_boot_address(0);
    let other = other(this);
    words.reset_core(other);
    words.request_stall(other);
}

/// The cores' owner: hart 1's start, park, stall and run. One exists per
/// image, on hart 0, which it never leaves (not `Send`).
pub struct Cores(ThisHart);

impl Cores {
    /// Take the cores' owner on hart 0, once: `None` on hart 1 or after it
    /// was taken. Its operations are refused once a fatal path holds the
    /// words.
    pub fn take() -> Option<Self> {
        (current_hart() == HpCore::Core0
            && OWNER
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok())
        .then_some(Self(PhantomData))
    }

    fn operate<T>(
        &mut self,
        operation: impl FnOnce(&mut CoreControlRegisters) -> T,
    ) -> Result<T, CoresError> {
        operate(&CORE_CONTROL, operation)
    }

    /// How hart 1 stands: held in reset with its clock gated, stalled by the
    /// hardware's report, or neither.
    ///
    /// # Errors
    /// No words are installed, or a fatal path holds them.
    pub fn hart1(&mut self) -> Result<Hart1, CoresError> {
        self.operate(|words| hart1(words))
    }

    /// Park hart 1 as ESP-IDF's single-core start does: request its stall,
    /// then gate its clocks and hold it in its global reset.
    ///
    /// # Errors
    /// No words are installed, or a fatal path holds them.
    pub fn park_hart1(&mut self) -> Result<(), CoresError> {
        self.operate(park_hart1)
    }

    /// Stall hart 1 and wait for the hardware to confirm it.
    ///
    /// # Errors
    /// No words are installed, a fatal path holds them, or the hardware did
    /// not confirm the stall within [`STALL_POLLS`].
    pub fn stall_hart1(&mut self) -> Result<(), CoresError> {
        self.operate(|words| stall(words, HpCore::Core1, STALL_POLLS))?
            .map_err(|Unconfirmed| CoresError::Unconfirmed)
    }

    /// Let hart 1 run and wait for the hardware to report it no longer
    /// stalled.
    ///
    /// # Errors
    /// No words are installed, a fatal path holds them, or the hardware did
    /// not confirm the run within [`STALL_POLLS`].
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
    /// No words are installed, a fatal path holds them, or the hardware did
    /// not confirm the run within [`STALL_POLLS`].
    #[cfg(feature = "cpu1-runs")]
    pub fn start_hart1(&mut self, entry: extern "C" fn() -> !) -> Result<(), CoresError> {
        self.operate(|words| {
            words.set_boot_address(0);
            run(words, HpCore::Core1, STALL_POLLS)?;
            words.release_core1();
            words.set_boot_address(entry as usize as u32);
            Ok(())
        })?
        .map_err(|Unconfirmed| CoresError::Unconfirmed)
    }
}

/// Run `operation` on the words of `cell`, as the cores' owner.
fn operate<W, T>(
    cell: &ClaimCell<W>,
    operation: impl FnOnce(&mut W) -> T,
) -> Result<T, CoresError> {
    cell.lend(operation).map_err(|claim| match claim {
        Claim::Empty => CoresError::NotInstalled,
        Claim::Lent | Claim::Kept => CoresError::Stopped,
    })
}

/// How hart 1 stands.
fn hart1<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &CoreControlRegisters<P, Q, R, S, T>,
) -> Hart1 {
    if words.core1_held() {
        Hart1::Held
    } else if words.is_stalled(HpCore::Core1) {
        Hart1::Stalled
    } else {
        Hart1::Released
    }
}

/// Request hart 1's stall, then hold it in reset with its clocks gated, as
/// ESP-IDF's single-core start leaves it.
pub(crate) fn park_hart1<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    words: &mut CoreControlRegisters<P, Q, R, S, T>,
) {
    words.request_stall(HpCore::Core1);
    words.hold_core1();
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
    /// The reset root installed no core-control words.
    NotInstalled,
    /// The other hart's fatal path holds the core-control words: it stops
    /// this hart.
    Lost,
    /// The words are lent: to an operation of the cores' owner, which this
    /// path may have interrupted, or to this hart's own fatal path it
    /// interrupted. It is not waited for.
    Busy,
    /// This hart's fatal path took the words before: a second stop.
    Reentered,
    /// The hardware did not confirm the other hart's stall within
    /// [`STALL_POLLS`].
    Unconfirmed,
}

/// The other hart stopped for the chip's reset: this hart's interrupts are
/// masked and the other hart is confirmed stopped. It prepares the reset
/// and gives no right to a block another owner holds: the stop does not
/// show that the other hart's bus transactions completed. It stays in its
/// fatal path, on its hart (not `Send`).
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
}

/// Stop the world from `fatal`'s path before its reset: hold the
/// core-control words for good, then stop the other hart and confirm it.
/// The path resets with the same proof afterwards ([`software_reset`]).
///
/// # Errors
/// [`StopError`]: no words are installed, the other hart's fatal path or the
/// cores' owner holds them, this hart stopped the world before, or the other
/// hart's stall was not confirmed. The caller keeps its record and resets
/// all the same.
pub fn stop_the_world<'a>(fatal: &Fatal<'a>) -> Result<StopTheWorld<'a>, StopError> {
    let hart = fatal.hart;
    let stopped = stop_with(&CORE_CONTROL, hart, STALL_POLLS)?;
    Ok(StopTheWorld {
        hart,
        stopped,
        _path: PhantomData,
        _hart: PhantomData,
    })
}

fn stop_with<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    cell: &ClaimCell<CoreControlRegisters<P, Q, R, S, T>>,
    hart: HpCore,
    polls: u32,
) -> Result<Stopped, StopError> {
    match cell.hold(key(hart), |words| stop_other(words, hart, polls)) {
        Ok(Held { value, first: true }) => value.map_err(|Unconfirmed| StopError::Unconfirmed),
        Ok(Held { first: false, .. }) => Err(StopError::Reentered),
        Err(Claim::Empty) => Err(StopError::NotInstalled),
        Err(Claim::Lent) => Err(StopError::Busy),
        Err(Claim::Kept) => Err(StopError::Lost),
    }
}

/// A fatal path that could not prepare the chip's reset: the other hart's
/// fatal path or a lent borrow holds the core-control words, or none are
/// installed. The path halts its masked hart, without returning to the code
/// it interrupted, until an armed watchdog resets the chip.
#[must_use = "the path halts its masked hart until a watchdog resets the chip"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AwaitWatchdog {
    /// Why the words could not be held.
    pub reason: StopError,
}

/// The reason the console's fatal note gives for a reset left to the
/// watchdog.
pub const RESET_UNPREPARED: &str = "reset-unprepared";

/// Reset the chip from `fatal`'s path, as the boot ROM expects it: UART0's
/// clock restored, no boot address for hart 1, the other hart reset and
/// stalled, then the system reset trigger and a terminal interrupt wait.
/// UART0's clock has its own cell, so every path restores it unless the other
/// hart's fatal path took it first to restore it itself.
///
/// A path that cannot hold the core-control words writes the console's
/// fatal note ([`RESET_UNPREPARED`]) if the block is free and returns
/// [`AwaitWatchdog`]: its hart then halts, masked, until an armed watchdog
/// resets the chip. No second owner of the words is made.
pub fn software_reset(fatal: Fatal<'_>) -> AwaitWatchdog {
    restore_uart0_with(&UART0_CLOCK, fatal.hart);
    match reset_with(&CORE_CONTROL, fatal.hart) {
        Ok(()) => loop {
            #[cfg(target_arch = "riscv32")]
            oer_riscv::wait_for_interrupt();
            #[cfg(not(target_arch = "riscv32"))]
            #[allow(
                clippy::disallowed_methods,
                reason = "the host has no WFI; this reset path is terminal"
            )]
            core::hint::spin_loop();
        },
        Err(reason) => {
            let _held_or_written =
                crate::usb_serial_jtag::fatal_note(&Note::runtime_fail(RESET_UNPREPARED));
            AwaitWatchdog { reason }
        }
    }
}

/// Restore UART0's clock for the boot ROM if `hart` holds the word; whether
/// it did.
fn restore_uart0_with<P: Placement>(
    cell: &ClaimCell<Uart0ClockRegisters<P>>,
    hart: HpCore,
) -> bool {
    cell.hold(key(hart), |clock| clock.restore_uart0_clock())
        .is_ok()
}

/// Prepare and trigger the reset holding the words, or say why they
/// could not be held.
fn reset_with<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>(
    cell: &ClaimCell<CoreControlRegisters<P, Q, R, S, T>>,
    hart: HpCore,
) -> Result<(), StopError> {
    match cell.hold(key(hart), |words| {
        prepare_reset(words, hart);
        oer_riscv::fence();
        words.request_system_reset();
    }) {
        Ok(Held { value: (), .. }) => Ok(()),
        Err(Claim::Empty) => Err(StopError::NotInstalled),
        Err(Claim::Lent) => Err(StopError::Busy),
        Err(Claim::Kept) => Err(StopError::Lost),
    }
}
