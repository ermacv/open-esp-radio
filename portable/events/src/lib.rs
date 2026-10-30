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
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Event sinks a caller lends an owner for one call.
//!
//! An owner that decides something (a dropped frame, a refused key, a
//! released reorder window) reports it as a typed event value to the sink its
//! caller passes to that call. The owner keeps neither the sink nor the event
//! and reads nothing back: what an event becomes (a count, a sum, the last
//! value, a record), when it is read and which time it is bound to belong to
//! the caller. Reporting needs no lock, atomic or interior mutability: the
//! sink is borrowed exclusively for the call.
//!
//! `()` is the sink that keeps nothing. Its [`EventSink::ENABLED`] is
//! `false`, so [`report`] never builds the event: no argument is evaluated
//! and, after monomorphization, the report point leaves no code.

/// Receives the events of the owner it is lent to.
///
/// # Events
///
/// An event is a `Copy` value of the owner's own type, so it carries no
/// affine owner (a buffer, a register block, a lease, a permit). Its fields
/// are values the owner computed or copies of what its caller supplied, such
/// as a caller's key; an owner's event type holds no borrow of frame bytes or
/// of state the owner keeps. A borrowed field keeps its lifetime: a sink
/// cannot retain the event beyond the data it borrows.
///
/// # Cost
///
/// `event` runs inside the owner's call, on the owner's context. A
/// production sink completes in bounded time: it does not wait, block, take a
/// lock, allocate, or do I/O; it may update its own values (a count, a sum,
/// the last value, a fixed buffer that drops on overflow). A host test sink
/// may record without these limits. `event` returns `()`, so a sink cannot
/// steer the owner; returning `()` proves none of the cost limits, which are
/// each implementation's obligation. The trait has no `dyn` form: every
/// report is a direct call.
pub trait EventSink<E: Copy> {
    /// Whether this sink receives events; [`report`] builds an event only
    /// when it does.
    const ENABLED: bool = true;

    /// Receive one event.
    fn event(&mut self, event: E);
}

/// The sink that keeps nothing.
impl<E: Copy> EventSink<E> for () {
    const ENABLED: bool = false;

    #[inline(always)]
    fn event(&mut self, _: E) {}
}

/// An owner lends its own loan on to a child.
impl<E: Copy, S: EventSink<E> + ?Sized> EventSink<E> for &mut S {
    const ENABLED: bool = S::ENABLED;

    #[inline(always)]
    fn event(&mut self, event: E) {
        (**self).event(event);
    }
}

/// Report the event `event` builds to `sink`. The closure runs only for an
/// enabled sink, so building the event costs nothing when the caller lent
/// `()`. It takes values the owner already computed: no frame parse, clock
/// read or search.
#[inline(always)]
pub fn report<E: Copy, S: EventSink<E> + ?Sized>(sink: &mut S, event: impl FnOnce() -> E) {
    if S::ENABLED {
        sink.event(event());
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]
mod tests;
