//! Capacity as values: the slot pools of one install.
//!
//! A port's capacity is never a refusal (`docs/architecture.md`,
//! Ownership). What the hardware holds at once (key slots, Block Ack slots,
//! a gate) is a set of slots the install mints once, through the [`Mint`]
//! that [`branded`] makes for an installation, branded by the install
//! (`'id`), its semantic kind `Kind`, and its sealed component `PoolPath`.
//! Tuple factories derive a distinct child path for every component, even
//! when the same kind occurs twice or inside a custom nested layout.
//!
//! A **slot** ([`Slot`]) is taken synchronously: a pool is an array of slots
//! its holder owns, so how many are left is in the types, and an empty pool
//! is an array of none, not a run-time `None`. Installing a resource moves a
//! slot into it, and removing the resource gives the slot back. A caller
//! counts its own slots: it plans against the same constant the backend
//! minted its pool with, so taking a slot is an invariant of that count
//! (popping the array it holds), never a refusal.
//!
//! A backend declares one pool for each kind in its `Pools` layout. Consuming
//! [`Mint::into_pools`] issues the whole set once; the factory cannot repeat
//! issuance with the same mint. The kind keeps the declared pools apart.
//!
//! No token is `Clone`, `Copy` or `Default`, and only a [`Mint`] makes
//! one, so no caller adds capacity:
//!
//! ```compile_fail,E0599
//! # use oer_protocols_radio_port::Slot;
//! fn copy<'id, K>(slot: &Slot<'id, K>) -> Slot<'id, K> {
//!     slot.clone()
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_protocols_radio_port::Slot;
//! fn forge<'id, Kind>() -> Slot<'id, Kind> {
//!     Slot { index: 0, brand: PhantomData, kind: PhantomData }
//! }
//! ```
//!
//! ```compile_fail,E0382
//! # use oer_protocols_radio_port::Slot;
//! fn install_twice<'id, Kind>(slot: Slot<'id, Kind>, install: impl Fn(Slot<'id, Kind>)) {
//!     install(slot);
//!     install(slot);
//! }
//! ```
//!
//! A slot of one install does not reach another, nor leaves its body for
//! `'static`; neither has an error code of its own (a lifetime error):
//!
//! ```compile_fail
//! # use oer_protocols_radio_port::Slot;
//! fn mix<'a, 'b, Kind>(slot: Slot<'a, Kind>) -> Slot<'b, Kind> {
//!     slot
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_protocols_radio_port::Slot;
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
//! # use oer_protocols_radio_port::Slot;
//! fn shrink<'a: 'b, 'b, K>(slot: Slot<'a, K>) -> Slot<'b, K> {
//!     slot
//! }
//! ```
//!
//! The compiling twins of the slot rules move a slot within its own install:
//!
//! ```
//! # use oer_protocols_radio_port::Slot;
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

use core::marker::PhantomData;

/// The brand of one install.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

mod mint;
mod path;
pub use mint::{Mint, PoolBuilder, PoolSet, Pools, Slots, branded};
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
/// resource gives it back. A slot [`Slots<K, N>`](Slots) minted has that
/// layout as its kind.
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
