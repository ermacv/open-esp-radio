#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! The vocabulary radio ports share: capacity as values ([`Mint`], [`Slot`],
//! [`PoolSet`]), the result of a port call ([`PortResult`], [`Poisoned`]),
//! the marker of lost events ([`EventsLost`]) and the radio clock
//! ([`ClockInfo`], [`RadioStamp`], [`ClockSample`], [`ClockError`]).

mod clock;
mod identity;
pub mod pool;
mod work;

pub use clock::{ClockInfo, ClockSample, RadioStamp};
pub use identity::{LocalId, SlotIdentity};
pub use pool::{Child, Mint, PoolBuilder, PoolPath, PoolSet, Pools, Root, Slot, Slots, branded};
pub use work::ClockError;

/// The result of a port call: `Ok(Ok(_))` served, `Ok(Err(refusal))`
/// refused with nothing changed, `Err(Poisoned)` a poisoned backend.
pub type PortResult<T, R, C> = Result<Result<T, R>, Poisoned<C>>;

/// The backend dropped events it could not hold.
///
/// The marker takes the place of the first dropped event: every event
/// reported before it was produced before the gap, and every event reported
/// after it was produced after every dropped one. It is reported once per
/// gap, and never silently discarded while the port lives; a gap the port's
/// uninstall leaves ends with its stream.
///
/// A backend reserves the slot of every terminal event, and of data its
/// protocol promises to deliver, when it admits the work, so the gap holds
/// only data the protocol does not promise (received frames, advertising
/// reports), which is gone; the consumer continues after it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// A poisoned backend: its state is unknown and only a reset restores it.
///
/// It is the outer error of commands and clock calls, returned at once.
/// `cause` is the backend's own
/// account of the fault, which portable code passes on without
/// interpreting it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Poisoned<C> {
    /// Why the backend's state is unknown.
    pub cause: C,
}

#[cfg(test)]
mod tests;
