//! Installation tokens that name their installation.
//!
//! An installation of one static identity is minted again after a
//! reinstallation, and its tokens have the same types as the earlier
//! installation's: a caller may still hold a token of the earlier one. Each
//! token the port's own pools mint is therefore an [`IssuedSlot`], which
//! names the generation of the installation that issued it
//! ([`Installation::generation`](oer_protocols_radio_port::Installation::generation)).
//! A pool yields a [`SlotGrant`]; only the backend that consumed the mint
//! holds it, and it issues the grant once with its installation's
//! generation. A backend refuses a token of an earlier installation at the
//! operation's acceptance point, with the token back.
//!
//! No caller makes or re-stamps an issued slot:
//!
//! ```compile_fail,E0451
//! use oer_protocols_ieee80211_contracts_port::IssuedSlot;
//! use oer_protocols_radio_port::Slot;
//! fn forge<'id, K>(slot: Slot<'id, K>) -> IssuedSlot<'id, K> {
//!     IssuedSlot { slot, generation: 7 }
//! }
//! ```
//!
//! Its compiling twin issues the slots a mint made:
//!
//! ```
//! use oer_protocols_ieee80211_contracts_port::{IssuedSlot, IssuedSlots};
//! use oer_protocols_radio_port::{Mint, Slots};
//! enum Kind {}
//! fn issue<'id>(mint: Mint<'id, IssuedSlots<Kind, 2>>) -> [IssuedSlot<'id, Slots<Kind, 2>>; 2] {
//!     mint.into_pools().issue(3)
//! }
//! ```

use core::marker::PhantomData;

use oer_protocols_radio_port::{
    BodyLoan, LocalId, Place, PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots,
};

/// A pool component of `N` issued slots of the kind `Kind`.
pub struct IssuedSlots<Kind, const N: usize>(PhantomData<fn() -> Kind>);

impl<Kind, const N: usize> PoolSet for IssuedSlots<Kind, N> {
    type Pools<'id, Path: PoolPath> = SlotGrant<'id, Kind, N, Path>;

    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        SlotGrant {
            slots: builder.into_pools::<Slots<Kind, N>>(),
        }
    }
}

/// The slots a mint made, before their backend issues them to its
/// installation.
#[must_use = "the backend issues the grant to its installation"]
pub struct SlotGrant<'id, Kind, const N: usize, Path: PoolPath = Root<LocalId<'id>>> {
    slots: [Slot<'id, Slots<Kind, N>, Path>; N],
}

impl<'id, Kind, const N: usize, Path: PoolPath> SlotGrant<'id, Kind, N, Path> {
    /// Issue every slot to the installation `generation`.
    pub fn issue(self, generation: u32) -> [IssuedSlot<'id, Slots<Kind, N>, Path>; N] {
        self.slots.map(|slot| IssuedSlot { slot, generation })
    }
}

/// One slot of kind `Kind` and the generation of the installation that
/// issued it. An [`IssuedSlots<Kind, N>`] issues kind `Slots<Kind, N>`, so its
/// [`place`](Self::place) is below `N` by its type.
#[must_use = "a slot is capacity; dropping it loses it until the next install"]
pub struct IssuedSlot<'id, Kind, Path: PoolPath = Root<LocalId<'id>>> {
    slot: Slot<'id, Kind, Path>,
    generation: u32,
}

impl<Kind, Path: PoolPath> IssuedSlot<'_, Kind, Path> {
    /// The slot's index in its pool.
    pub const fn index(&self) -> u8 {
        self.slot.index()
    }

    /// The generation of the installation that issued it.
    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

impl<Kind, const N: usize, Path: PoolPath> IssuedSlot<'_, Slots<Kind, N>, Path> {
    /// The slot's place in its pool of `N`.
    pub const fn place(&self) -> Place<N> {
        self.slot.place()
    }
}

impl<Kind, Path: PoolPath> core::fmt::Debug for IssuedSlot<'_, Kind, Path> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("IssuedSlot")
            .field("index", &self.slot.index())
            .field("generation", &self.generation)
            .finish()
    }
}

impl<Kind, Path: PoolPath> IssuedSlot<'static, Kind, Path> {
    /// Move the same slot across its backend's private body boundary.
    pub fn lend<'p, S>(self, loan: &BodyLoan<'p, Path::Identity, S>) -> IssuedSlot<'p, Kind, Path> {
        IssuedSlot {
            slot: loan.lend_slot(self.slot),
            generation: self.generation,
        }
    }
}

impl<'p, Kind, Path: PoolPath> IssuedSlot<'p, Kind, Path> {
    /// Move the same slot across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> IssuedSlot<'static, Kind, Path> {
        IssuedSlot {
            slot: loan.reclaim_slot(self.slot),
            generation: self.generation,
        }
    }
}

/// The first of `generations` that is not `installation`: a token of an
/// earlier installation among an operation's tokens.
pub fn first_stale(installation: u32, generations: &[u32]) -> Option<u32> {
    generations
        .iter()
        .copied()
        .find(|generation| *generation != installation)
}
