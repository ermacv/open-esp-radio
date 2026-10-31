#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
//! The Espressif family's platform contracts. A chip HAL implements them on
//! its own types, so family crates name a contract, never a chip, and the
//! dependency runs from the chip to the family.
//!
//! The HAL also selects its chip through this package's chip feature (an id
//! of `oer_espressif_facts::tracked::ids`), which enables no dependency. The
//! build script then generates the chip's hart geometry ([`HARTS`],
//! [`BOOT_HART`], the checked [`Hart`]) with the PAC's generator over the
//! same facts and publishes the id as this package's `chip` metadata. A
//! build without a chip has no geometry, no [`Platform`] and no
//! [`ExecutorPlatform`].
//!
//! The contracts:
//!
//! - [`EarlyText`]: the early console writer;
//! - [`Platform`]: the fatal path's mask and proof, the stop of every other
//!   hart and the chip's software reset;
//! - [`ExecutorPlatform`]: the executor's source identities, its pending and
//!   debugger reads and its owned blocks: the software interrupt lines
//!   ([`SoftwareLine`]), the counter owner (the binding requires its family
//!   driver's latch atoms), the comparators ([`Comparators`]) and their alarm
//!   parts ([`AlarmPart`]);
//! - [`SecondaryStart`]: the release of a secondary hart into the staged
//!   runtime's generated entry ([`SecondaryEntry`]), consuming the root's
//!   one-shot launch permit, and the retirement of its ROM boot word.

use oer_mmio::InterruptSource;

pub use oer_espressif_staged_layout::handoff::SecondaryEntry;

#[cfg(chip_geometry)]
mod geometry {
    include!(concat!(env!("OUT_DIR"), "/geometry.rs"));
}
#[cfg(chip_geometry)]
pub use geometry::{BOOT_HART, HARTS, Hart, InvalidHart, SecondaryHart};

/// The early console writer: what a record writes before any driver owns
/// the console.
pub trait EarlyText {
    /// Writes `bytes` as they are, or drops and counts them while another
    /// record holds the console; it never waits beyond the writer's
    /// per-record budget. `false` once a driver or the fatal path took the
    /// console for good.
    fn text(bytes: &[u8]) -> bool;
}

/// One software interrupt line: one level bit that wakers raise from every
/// context and the line's handler lowers.
pub trait SoftwareLine {
    /// Raise the line; its raise has reached the interrupt matrix when this
    /// returns.
    fn raise(&self);
    /// Lower the line.
    fn lower(&self);
}

/// A system timer comparator; every value names an existing target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparator {
    Zero,
    One,
    Two,
}

impl Comparator {
    /// The comparator of index `index`.
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            2 => Some(Self::Two),
            _ => None,
        }
    }
}

/// A comparator's alarm part: its handler acknowledges its own interrupt.
pub trait AlarmPart: InterruptSource {
    /// The comparator whose interrupt the part serves.
    const COMPARATOR: Comparator;
    /// Strobe the comparator's acknowledgement bit alone.
    fn acknowledge(&mut self);
}

/// The system timer's comparators, which one writer owns.
pub trait Comparators {
    /// Bind `alarm`'s comparator in one-shot mode before its handler's
    /// route is enabled: the comparator disabled, a stale interrupt
    /// acknowledged, then its interrupt enabled.
    fn bind<A: AlarmPart>(&mut self, alarm: &mut A);

    /// Load `comparator`'s target `ticks` and enable it; `None` only
    /// disables the comparator.
    fn program(&mut self, comparator: Comparator, ticks: Option<u64>);
}

/// A fatal path that could not prepare the chip's reset: its hart halts,
/// masked, until an armed watchdog resets the chip.
#[must_use = "the path halts its masked hart until a watchdog resets the chip"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AwaitWatchdog<E> {
    /// Why the reset could not be prepared.
    pub reason: E,
}

/// The fatal path's primitives: the mask of the failing hart and its proof,
/// the stop of every other hart and the chip's software reset. The family's
/// staged runtime keeps the policy (what a path records and when it resets);
/// the chip HAL holds the proof and the owned words.
#[cfg(chip_geometry)]
pub trait Platform: EarlyText + 'static {
    /// Proof that this hart runs a fatal path with its interrupts masked;
    /// it stays on its hart and in the closure it is lent to.
    type Fatal<'a>;
    /// Every other hart stopped for the chip's reset, under a fatal path.
    type Stop<'a>;
    /// Why a fatal path did not stop the world.
    type StopError: Copy;

    /// Mask this hart's interrupts, lend `path` its proof, then halt if it
    /// returns.
    fn fatal(path: impl for<'a> FnOnce(Self::Fatal<'a>)) -> !;

    /// The hart that fails, or the raw identity that names no hart.
    ///
    /// # Errors
    ///
    /// [`InvalidHart`] when the architectural identity is outside the
    /// chip's geometry.
    fn fatal_hart(fatal: &Self::Fatal<'_>) -> Result<Hart, InvalidHart>;

    /// Stop every other hart before the chip's reset and confirm it.
    ///
    /// # Errors
    ///
    /// [`Self::StopError`] when the stop could not be taken or confirmed;
    /// the path records and resets all the same.
    fn stop_the_world<'a>(fatal: &Self::Fatal<'a>) -> Result<Self::Stop<'a>, Self::StopError>;

    /// Reset the chip from `fatal`'s path. It returns only when the reset
    /// could not be prepared and is left to the armed watchdog.
    fn software_reset(fatal: Self::Fatal<'_>) -> AwaitWatchdog<Self::StopError>;
}

/// A secondary hart's release, which the staged runtime's launch orders
/// around its handshake. The chip HAL checks the hart's preconditions,
/// disconnects its interrupt routes and releases it into `entry`, consuming
/// the root's one-shot `Permit`; the runtime then waits for the entry, retires
/// the boot word and acknowledges. No other path starts a hart.
#[cfg(chip_geometry)]
pub trait SecondaryStart: Platform {
    /// The cores' owner, on the boot hart.
    type Cores;
    /// The root's one-shot authority to launch a secondary hart.
    type Permit;
    /// What the preconditions read besides the owned words (the reset causes).
    type Preconditions: Copy;
    /// A released hart's retained owners (its route bank).
    type Released;
    /// A refused or unconfirmed step, with its observations.
    type StartError: Copy;

    /// Check the preconditions and release `entry`'s hart into it; the boot
    /// word is written last.
    ///
    /// # Errors
    ///
    /// A refused precondition leaves the hart held and its boot word zero; an
    /// unconfirmed release step returns its observation.
    fn release(
        cores: &mut Self::Cores,
        permit: Self::Permit,
        preconditions: Self::Preconditions,
        entry: SecondaryEntry,
    ) -> Result<Self::Released, Self::StartError>;

    /// Clear the released hart's boot word and read it back as zero, so the
    /// ROM cannot enter the hart's entry again.
    ///
    /// # Errors
    ///
    /// The word read back nonzero, or the words are unavailable.
    fn retire_boot_word(
        cores: &mut Self::Cores,
        released: &mut Self::Released,
    ) -> Result<(), Self::StartError>;

    /// A refused step's code and observations, for the retained record.
    fn observations(error: Self::StartError) -> [u32; 3];
}

/// The executor's platform: the source identities of the software
/// interrupt lines and the comparators, the pending and debugger reads of a
/// checked hart and the blocks the executor owns. No vendor interrupt enum
/// or singleton crosses this boundary.
#[cfg(chip_geometry)]
pub trait ExecutorPlatform {
    /// Sources of the software interrupt lines 0 through 3, in line order.
    const SOFTWARE_SOURCES: [u16; 4];
    /// Sources of comparators 0 through 2, in comparator order.
    const ALARM_SOURCES: [u16; 3];
    /// The counter's ticks per microsecond.
    const TICKS_PER_MICROSECOND: u64;
    /// Software interrupt line 0's owner.
    type Line0: SoftwareLine + Sync + 'static;
    /// Software interrupt line 1's owner.
    type Line1: SoftwareLine + Sync + 'static;
    /// Software interrupt line 2's owner.
    type Line2: SoftwareLine + Sync + 'static;
    /// Software interrupt line 3's owner.
    type Line3: SoftwareLine + Sync + 'static;
    /// The counter's owner, shared by every context. The executor binding
    /// requires its family timer driver's latch port on this owner.
    type Counter: Sync + 'static;
    /// The comparators' owner, which the time driver's writer holds.
    type Comparators: Comparators;
    /// Whether `source` is pending at `hart`'s interrupt matrix, without
    /// changing its route.
    fn is_pending(hart: Hart, source: u16) -> bool;
    /// Whether a debugger is attached to `hart`: a wait for an interrupt
    /// would stall its access.
    fn debugger_connected(hart: Hart) -> bool;
}

#[cfg(test)]
mod tests;
