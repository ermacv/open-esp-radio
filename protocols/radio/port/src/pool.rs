//! Capacity as values: the slot pools and permit pools of one install.
//!
//! A port's capacity is never a refusal (`docs/architecture.md`,
//! Ownership). What the hardware holds at once (key slots, Block Ack slots, accept-list
//! entries, a transmit queue, the one operation of a radio) is a set of
//! tokens the install mints once, through the [`Mint`] that [`branded`]
//! makes for each block's `run_port` to hand its backend, branded by the install
//! (`'id`), its semantic kind `Kind`, and its sealed component `PoolPath`.
//! Tuple factories derive a distinct child path for every component, even
//! when the same kind occurs twice or inside a custom nested layout.
//!
//! - A **slot** ([`Slot`]) is taken synchronously: a pool is an array of
//!   slots its holder owns, so how many are left is in the types, and an
//!   empty pool is an array of none, not a run-time `None`. Installing a
//!   resource moves a slot into it, and removing the resource gives the
//!   slot back.
//! - A **permit** ([`Permit`]) is awaited: a [`PermitPool`] holds a fixed
//!   number, and taking one waits while all are held, never refused. A
//!   submission moves its permit into the backend, which keeps the pool in
//!   its held part of the install (`PortBackend::Held`) and takes the
//!   permit back when the work ends, before it hands the terminal event
//!   out: released when the air finished, not when a handle drops, and
//!   never lost with an event dropped unread; a cancel is only a request. In which order waiting
//!   takers are served is the pool's documented policy (first in, first out
//!   by default), which the conformance suite checks.
//! - A **seat** ([`Seat`]) is a place in a permit pool's line: a taker
//!   lends its seat to [`PermitPool::take`] for as long as it waits, so a
//!   taker without one cannot exist, and a pool whose line has a place per
//!   seat never overflows. [`Permits`] issues a kind's seats with the
//!   [`Line`] that sizes its pools, so the line's capacity is a value, as
//!   every other capacity is. A line is a copyable receipt: several pools
//!   whose takers are the same seats (a transmit queue's pool each) take
//!   it, each with a place for every seat, and since a seat waits in one
//!   take at a time, none of them overflows. The seat stays with its taker: a take that
//!   is served or dropped ends the loan, and the taker waits again with
//!   the same seat (an A-MPDU builder's sequential buffer takes). What a
//!   seat stands for beyond its kind (an interface's role and access
//!   category) is its value: the backend that minted it maps its
//!   [`index`](Seat::index), so the pool and its take are generic over the
//!   kind alone.
//!
//! A caller counts its own slots: it plans against the same constant the
//! backend minted its pool with, so taking a slot synchronously is an
//! invariant of that count (popping the array it holds), never a refusal.
//!
//! Body-facing tokens retain an invariant body loan. A backend can privately
//! convert only capacity tokens between that loan and owned Id state through
//! [`BodyLoan`]; the owned state carries no P1 reference. Their component
//! paths and [`index`](Permit::index) remain unchanged through that handoff.
//!
//! This package owns no state and never waits: it states the tokens and the
//! rules. A pool's free list, its waiters and their wake-ups belong to a
//! runtime crate or to the backend that keeps it. The backend never refuses
//! for capacity: a caller takes synchronously from the slots it holds, and
//! one that holds none turns that into its protocol's decision (an access
//! point refuses an association).
//!
//! A backend declares one pool for each kind in its `Pools` layout. Consuming
//! [`Mint::into_pools`] issues the whole set once; the factory cannot repeat
//! issuance with the same mint. The kind keeps the declared pools apart.
//!
//! No token is `Clone`, `Copy` or `Default`, and only a [`Mint`] makes
//! one, so no caller adds capacity:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::Slot;
//! fn copy<'id, K>(slot: &Slot<'id, K>) -> Slot<'id, K> {
//!     slot.clone()
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_radio_port::Slot;
//! fn forge<'id, Kind>() -> Slot<'id, Kind> {
//!     Slot { index: 0, brand: PhantomData, kind: PhantomData }
//! }
//! ```
//!
//! ```compile_fail,E0382
//! # use oer_radio_port::Slot;
//! fn install_twice<'id, Kind>(slot: Slot<'id, Kind>, install: impl Fn(Slot<'id, Kind>)) {
//!     install(slot);
//!     install(slot);
//! }
//! ```
//!
//! A seat, too, is neither cloned nor forged, and no caller makes a line:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::Seat;
//! fn copy<'id, K>(seat: &Seat<'id, K>) -> Seat<'id, K> {
//!     seat.clone()
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_radio_port::Seat;
//! fn forge<'id, K>() -> Seat<'id, K> {
//!     Seat { index: 0, brand: PhantomData, kind: PhantomData }
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_radio_port::Line;
//! fn forge<'id, K>() -> Line<'id, K, 4> {
//!     Line { brand: PhantomData, kind: PhantomData }
//! }
//! ```
//!
//! A take needs a seat, of the pool's own kind:
//!
//! ```compile_fail,E0061
//! # use oer_radio_port::PermitPool;
//! async fn unseated<'id, K>(pool: &impl PermitPool<'id, K>) {
//!     let _ = pool.take().await;
//! }
//! ```
//!
//! ```compile_fail,E0308
//! # use oer_radio_port::{PermitPool, Seat};
//! async fn elsewhere<'id, K, J>(pool: &impl PermitPool<'id, K>, seat: &mut Seat<'id, J>) {
//!     let _ = pool.take(seat).await;
//! }
//! ```
//!
//! Their compiling twin takes with a seat of the pool's kind and keeps it
//! for the next take:
//!
//! ```
//! # use oer_radio_port::{Permit, PermitPool, Seat};
//! async fn twice<'id, K>(
//!     pool: &impl PermitPool<'id, K>,
//!     mut seat: Seat<'id, K>,
//! ) -> (Permit<'id, K>, Permit<'id, K>, Seat<'id, K>) {
//!     let first = pool.take(&mut seat).await;
//!     let second = pool.take(&mut seat).await;
//!     (first, second, seat)
//! }
//! ```
//!
//! A slot of one install does not reach another, nor leaves its body for
//! `'static`; neither has an error code of its own (a lifetime error):
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn mix<'a, 'b, Kind>(slot: Slot<'a, Kind>) -> Slot<'b, Kind> {
//!     slot
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn keep<'id, Kind>(slot: Slot<'id, Kind>) -> Slot<'static, Kind> {
//!     slot
//! }
//! ```
//!
//! The brand is invariant: even a brand that outlives another does not
//! shrink to it, so a slot of an outer install never passes for an inner
//! one's:
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn shrink<'a: 'b, 'b, K>(slot: Slot<'a, K>) -> Slot<'b, K> {
//!     slot
//! }
//! ```
//!
//! The compiling twins of the slot rules move a slot within its own install:
//!
//! ```
//! # use oer_radio_port::Slot;
//! fn install_once<'id, Kind>(slot: Slot<'id, Kind>, install: impl Fn(Slot<'id, Kind>)) {
//!     install(slot);
//! }
//! fn same<'a, Kind>(slot: Slot<'a, Kind>) -> Slot<'a, Kind> {
//!     slot
//! }
//! fn index<Kind>(slot: &Slot<'_, Kind>) -> u8 {
//!     slot.index()
//! }
//! ```

use core::{future::Future, marker::PhantomData};

/// The brand of one install.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

mod loan;
mod mint;
pub use loan::BodyLoan;
mod path;
pub use mint::{
    Mint, PermitSupply, PermitTokens, Permits, PoolBuilder, PoolSet, Pools, Slots, branded,
};
pub use path::{Child, PoolPath, Root};

/// The index of a token of a pool of at most 256, which `Mint` asserts at
/// compile time.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a pool holds at most 256 tokens"
)]
const fn index_of(index: usize) -> u8 {
    index as u8
}

/// One slot of component `Path` of an install, of kind `Kind`: the capacity of one
/// installed resource. Installing a resource moves it in; removing the
/// resource gives it back.
#[must_use = "a slot is capacity; dropping it loses it until the next install"]
pub struct Slot<'id, Kind, Path: PoolPath = Root<crate::LocalId<'id>>> {
    index: u8,
    brand: Brand<'id>,
    kind: PhantomData<fn() -> (Kind, Path)>,
}

impl<Kind, Path: PoolPath> Slot<'_, Kind, Path> {
    /// The slot's index in its pool, for the backend that places the
    /// resource.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// One permit of a pool of the kind `Kind`: one unit of capacity a piece of
/// work holds until it ends; the backend that keeps the pool gives it back
/// before it hands the work's terminal out.
#[must_use = "a permit goes back to its pool through `PermitPool::release`"]
pub struct Permit<'id, Kind, Path: PoolPath = Root<crate::LocalId<'id>>> {
    index: u8,
    brand: Brand<'id>,
    kind: PhantomData<fn() -> (Kind, Path)>,
}

impl<Kind, Path: PoolPath> Permit<'_, Kind, Path> {
    /// The permit's index in its pool, for the backend that places the
    /// work.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// One place in the line of a permit pool of the kind `Kind`, which its
/// taker lends to [`PermitPool::take`] while it waits and keeps across
/// takes. What it stands for beyond its kind (an interface's role and
/// access category) is its index, which the backend that minted it maps.
#[must_use = "a seat is a place in a pool's line; dropping it loses it until the next install"]
pub struct Seat<'id, Kind, Path: PoolPath = Root<crate::LocalId<'id>>> {
    index: u8,
    brand: Brand<'id>,
    kind: PhantomData<fn() -> (Kind, Path)>,
}

impl<Kind, Path: PoolPath> Seat<'_, Kind, Path> {
    /// The seat's index in its line: the place the pool holds for it, and
    /// what the backend that minted it maps to its holder.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// The receipt for waiter domain `Path`: proof that
/// [`Mint`] made exactly its `N` seats of kind `Kind`, which sizes a pool's line to hold a
/// place for each, so a pool never overflows and serves every taker in
/// order. It is a copyable receipt, not capacity: each pool whose takers
/// are these seats takes it, and a seat waits in one take at a time.
#[must_use = "a line sizes its permit pools"]
pub struct Line<'id, Kind, const N: usize, Path: PoolPath = Root<crate::LocalId<'id>>> {
    brand: Brand<'id>,
    kind: PhantomData<fn() -> (Kind, Path)>,
}

impl<Kind, const N: usize, Path: PoolPath> Clone for Line<'_, Kind, N, Path> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Kind, const N: usize, Path: PoolPath> Copy for Line<'_, Kind, N, Path> {}

impl<Kind, const N: usize, Path: PoolPath> Line<'_, Kind, N, Path> {
    /// The places in the line, one per seat.
    pub const fn places(&self) -> usize {
        N
    }
}

/// A pool of the permits of one install of the kind `Kind`, which a runtime
/// crate or a backend keeps; it lives in one task, shared by reference by
/// the port's body and its phases, never across executor tasks. Its line
/// holds a place for each seat of its `Waiters` path, which [`Permits`] issued
/// with its [`Line`]. `Pool` independently identifies the returned permits;
/// multiple pools may share a waiter domain without sharing capacity.
pub trait PermitPool<
    'id,
    Kind,
    Pool: PoolPath = Root<crate::LocalId<'id>>,
    Waiters: PoolPath = Pool,
>
{
    /// Take a permit with `seat`, waiting while every permit is held, never
    /// refused, in the order the pool documents (first in, first out by
    /// default). The future borrows the seat while it waits: dropping it
    /// gives up the seat's place, and the seat stays with its taker.
    fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'id, Kind, Waiters>,
    ) -> impl Future<Output = Permit<'id, Kind, Pool>> + 'a;

    /// Give a permit back: the backend that keeps the pool calls it when the
    /// work that held the permit ended, before it hands the terminal out,
    /// or when a taker drops a permit it never submitted; the next waiting
    /// taker resumes.
    fn release(&self, permit: Permit<'id, Kind, Pool>);
}
