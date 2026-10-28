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
//! build without a chip has no geometry and no [`ExecutorPlatform`].
//!
//! The contracts:
//!
//! - [`EarlyText`]: the early console writer;
//! - [`ExecutorPlatform`]: the executor's source identities, its pending and
//!   debugger reads and its owned blocks: the software interrupt lines
//!   ([`SoftwareLine`]), the counter ([`SystemCounter`]), the comparators
//!   ([`Comparators`]) and their alarm parts ([`AlarmPart`]).

use oer_mmio::InterruptSource;

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

/// The counter's latch did not settle within its bound: a fault of the
/// system timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatchStuck;

/// A counter read that preempting strobes kept from verifying.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unverified {
    /// A count no earlier read exceeds and no later read undercuts.
    pub floor: u64,
}

/// The system timer's counter, which every context reads.
pub trait SystemCounter {
    /// Strobe the latch once and check that it settles within its bound,
    /// before any context reads the time.
    ///
    /// # Errors
    ///
    /// [`LatchStuck`] when the latch never turns valid.
    fn check(&self) -> Result<(), LatchStuck>;

    /// The counter's current value, in ticks, within the read's bounds.
    ///
    /// # Errors
    ///
    /// [`Unverified`], with its floor, when no bounded round verified a
    /// count.
    fn read(&self) -> Result<u64, Unverified>;
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
    /// The counter's owner, shared by every context.
    type Counter: SystemCounter + Sync + 'static;
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
