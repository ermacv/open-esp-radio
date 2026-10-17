#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Shared radio capacity, lifecycle and operation contracts.
//!
//! [`run_port`] consumes an [`Installed`] backend and its independent driver.
//! Its [`Owner`] borrows the backend and consumes each typestate transition.
//! [`Mint::into_pools`] issues each installation's complete capacity once.
//! The driver owns only unbranded state: queues containing branded slots and
//! in-flight work belong in [`PortBackend::Held`], borrowed by the driver.
//! See [`lifecycle`] for retirement, poisoning and cancellation.

#[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
extern crate std;

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod lifecycle;
pub mod pool;
mod relation;
mod work;

pub use lifecycle::*;
pub use work::*;

pub use pool::{
    Child, Line, Mint, Permit, PermitPool, PermitSupply, PermitTokens, Permits, PoolBuilder,
    PoolPath, PoolSet, Pools, Root, Seat, Slot, Slots, branded,
};
pub use relation::{
    ClockInfo, ClockRelation, ClockSample, Generation, GenerationSource, Projected,
    ProjectionError, RadioStamp, RelationRate, RelationSample, Stamp,
};

/// The result of a port call: `Ok(Ok(_))` served, `Ok(Err(refusal))`
/// refused with nothing changed, `Err(Poisoned)` a poisoned backend.
pub type PortResult<T, R, C> = Result<Result<T, R>, Poisoned<C>>;

/// The final end of an installation's event stream.
///
/// This is data, carrying only the backend's cause. It grants no hardware
/// authority and carries no owners or affine proofs. Undelivered owners stay
/// in the [`Quarantined`] retained by [`PortEnd`]. Every later
/// [`PortPhase::next_event`] returns the same variant and cause.
///
/// | Backend outcome | Event-stream end |
/// | --- | --- |
/// | [`Abandon::Stopped`] or an ordinary fault | `Drained`, after draining all terminals |
/// | [`Abandon::Unconfirmed`] | `Quarantined`, without waiting or acquiring the ISR's mutex |
/// | Terminal accounting violation | `Quarantined`, with the admitted owners retained |
///
/// Accounting quarantine does not imply [`Abandon::Unconfirmed`]. The
/// subsequent abandonment can still prove [`Abandon::Stopped`]; delivery
/// accounting and hardware quiescence are independent outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventEnd<C> {
    /// Every terminal of admitted work has been delivered through
    /// [`PortPhase::next_event`]. A missing terminal violates the backend
    /// contract; a body can detect it from its outstanding work.
    Drained {
        /// Why the event stream ended.
        cause: C,
    },
    /// Undelivered terminals and their owners remain quarantined in
    /// [`PortEnd`]. The body must fail-stop without attempting further
    /// draining or ordinary teardown.
    Quarantined {
        /// Why the event stream ended.
        cause: C,
    },
}

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
/// The event consumer instead reports [`EventEnd`] to distinguish drained
/// work from quarantined owners. `cause` is the backend's own
/// account of the fault, which portable code passes on without
/// interpreting it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Poisoned<C> {
    /// Why the backend's state is unknown.
    pub cause: C,
}

#[cfg(test)]
mod tests;
