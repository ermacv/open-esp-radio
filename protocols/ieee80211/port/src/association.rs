//! Association-scoped CCMP policy and the complete vacant key bank.
//!
//! An installation mints one owner and all of its PTK/GTK slots together.
//! Starting fixes SPP once; every installation borrows that owner. The bank's
//! array lengths and slot brands prove that ending has recovered every slot,
//! including those returned by `KeyRemoved`. Forgetting any slot can only
//! prevent a new association. Poisoned removal never returns one.
//!
//! `end_association` is a value transition, with no command or terminal. A
//! backend applies the immutable decision before publishing the association's
//! first key. Once all slots are vacant, CCMP cannot use its old decision;
//! the next association supplies the new one. Backend-specific evidence for
//! this boundary belongs with that backend.

use crate::InterfaceRole;
use core::marker::PhantomData;
use oer_ieee80211_wire::security::Spp;
use oer_radio_port::{Child, PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots};

/// Capacity of each PTK/GTK return collection retained by the portable station.
/// Backend banks remain generic: this bounds station bookkeeping, not AP slots.
pub const KEY_BANK_CAPACITY: usize = 8;

struct OwnerKind<R, const P: usize, const G: usize>(PhantomData<R>);
struct PairwiseKind<R, const P: usize, const G: usize>(PhantomData<R>);
struct GroupKind<R, const P: usize, const G: usize>(PhantomData<R>);

mod sealed {
    /// Only this crate's slot and pool types bind a capacity.
    pub trait Sealed {}
}

/// The key slots one association domain's pool mints: its capacity, known
/// at compile time.
pub trait KeyPool {
    /// The PTK slots.
    const PAIRWISE_SLOTS: usize;
    /// The GTK slots, including slots unused by a consumer.
    const GROUP_SLOTS: usize;
}

impl<R, const P: usize, const G: usize> KeyPool for AssociationPool<R, P, G> {
    const PAIRWISE_SLOTS: usize = P;
    const GROUP_SLOTS: usize = G;
}

/// A key slot, which names the pool that minted it. Only
/// [`PairwiseKeySlot`] and [`GroupKeySlot`] implement it, so a backend's
/// declared [`KeyTypes::Pool`](crate::KeyTypes::Pool) is the pool its slots
/// actually come from: a declaration of another size does not compile.
///
/// ```compile_fail,E0271
/// use oer_ieee80211_port::{Station, association::*};
/// fn declared<S: AssociationSlot<Pool = AssociationPool<Station, 1, 4>>>() {}
/// declared::<PairwiseKeySlot<'static, Station, 2, 2>>();
/// ```
///
/// ```
/// use oer_ieee80211_port::{Station, association::*};
/// fn declared<S: AssociationSlot<Pool = AssociationPool<Station, 2, 2>>>() {}
/// declared::<PairwiseKeySlot<'static, Station, 2, 2>>();
/// declared::<GroupKeySlot<'static, Station, 2, 2>>();
/// ```
pub trait AssociationSlot: sealed::Sealed {
    /// The pool that minted the slot.
    type Pool: KeyPool;
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath> sealed::Sealed
    for PairwiseKeySlot<'_, R, P, G, Path>
{
}
impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath> AssociationSlot
    for PairwiseKeySlot<'_, R, P, G, Path>
{
    type Pool = AssociationPool<R, P, G>;
}
impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath> sealed::Sealed
    for GroupKeySlot<'_, R, P, G, Path>
{
}
impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath> AssociationSlot
    for GroupKeySlot<'_, R, P, G, Path>
{
    type Pool = AssociationPool<R, P, G>;
}

/// The receive Block Ack slots of an install: `N` slots of kind `K`, each a
/// [`BlockAckSlot`] that names `N`. It is the only pool those slots come
/// from, so its `N` is the capacity, known at compile time.
pub struct BlockAckSlots<K, const N: usize>(PhantomData<fn() -> K>);

impl<K, const N: usize> PoolSet for BlockAckSlots<K, N> {
    type Pools<'id, Path: PoolPath> = [BlockAckSlot<'id, K, N, Path>; N];
    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        builder
            .into_pools::<Slots<K, N>>()
            .map(|slot| BlockAckSlot { slot })
    }
}

/// One receive Block Ack slot of a pool of `N`: an agreement holds it.
/// Only [`BlockAckSlots`] mints it.
#[must_use = "a Block Ack slot is capacity; dropping it loses it until the next install"]
pub struct BlockAckSlot<'id, K, const N: usize, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>>
{
    slot: Slot<'id, K, Path>,
}

impl<K, const N: usize, Path: PoolPath> BlockAckSlot<'_, K, N, Path> {
    /// The slot's index in its pool, for the backend that places the
    /// agreement.
    pub fn index(&self) -> u8 {
        self.slot.index()
    }
}

/// The pool of an install's receive Block Ack slots, with its capacity and
/// slot type. Only [`BlockAckSlots`] implements it, so
/// [`PhaseTypes::BlockAckPool`](crate::PhaseTypes::BlockAckPool) names the
/// slots the backend mints: a slot of a pool of another size is another
/// type.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_port::association::*;
/// type Declared = BlockAckSlots<u8, 4>;
/// fn take<'id>(_: <Declared as RxBlockAckPool>::Slot<'id, oer_radio_port::Root<oer_radio_port::LocalId<'id>>>) {}
/// fn give<'id>(minted: BlockAckSlot<'id, u8, 2>) {
///     take(minted)
/// }
/// ```
///
/// ```
/// use oer_ieee80211_port::association::*;
/// type Declared = BlockAckSlots<u8, 2>;
/// fn take<'id>(_: <Declared as RxBlockAckPool>::Slot<'id, oer_radio_port::Root<oer_radio_port::LocalId<'id>>>) {}
/// fn give<'id>(minted: BlockAckSlot<'id, u8, 2>) {
///     take(minted)
/// }
/// ```
pub trait RxBlockAckPool: sealed::Sealed {
    /// The kind of the slots.
    type Kind;
    /// The slots the pool mints.
    const LEN: usize;
    /// One slot of the pool, of the install component `Path`.
    type Slot<'id, Path: PoolPath>;
}

impl<K, const N: usize> sealed::Sealed for BlockAckSlots<K, N> {}
impl<K, const N: usize> RxBlockAckPool for BlockAckSlots<K, N> {
    type Kind = K;
    const LEN: usize = N;
    type Slot<'id, Path: PoolPath> = BlockAckSlot<'id, K, N, Path>;
}

/// One independently configured association domain and all of its key slots.
/// Interface-scoped hardware assigns one domain to the whole interface.
pub struct AssociationPool<R, const P: usize, const G: usize>(PhantomData<R>);

impl<R: InterfaceRole, const P: usize, const G: usize> PoolSet for AssociationPool<R, P, G> {
    type Pools<'id, Path: PoolPath> = VacantAssociation<'id, R, P, G, Path>;
    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build time"))]
            const fn check(pairwise: usize, group: usize) {
                if pairwise == 0 || pairwise > 256 || group > 256 {
                    panic!("association slots must fit their pool indices");
                }
            }
            check(P, G);
        }
        let ([owner], pairwise, group) = builder.into_pools::<(
            Slots<OwnerKind<R, P, G>, 1>,
            Slots<PairwiseKind<R, P, G>, P>,
            Slots<GroupKind<R, P, G>, G>,
        )>();
        VacantAssociation {
            start: AssociationStart { _owner: owner },
            bank: VacantKeyBank {
                pairwise: pairwise.map(|slot| PairwiseKeySlot { slot }),
                group: group.map(|slot| GroupKeySlot { slot }),
            },
        }
    }
}

/// The unique permission to select the next association's mode.
#[must_use]
pub struct AssociationStart<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    _owner: Slot<'id, OwnerKind<R, P, G>, Child<Path, 0>>,
}

/// One immutable SPP decision. Its brand also identifies every key slot.
#[must_use]
pub struct Association<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    start: AssociationStart<'id, R, P, G, Path>,
    spp: Spp,
}

/// All slots of this domain are vacant; none remains installed or in removal.
#[must_use]
pub struct VacantAssociation<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    start: AssociationStart<'id, R, P, G, Path>,
    bank: VacantKeyBank<'id, R, P, G, Path>,
}

/// A vacant PTK slot. Only its pool and a key-removal terminal produce it.
#[must_use]
pub struct PairwiseKeySlot<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    slot: Slot<'id, PairwiseKind<R, P, G>, Child<Path, 1>>,
}
/// A vacant GTK slot, from the same association domain as its PTK slots.
#[must_use]
pub struct GroupKeySlot<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    slot: Slot<'id, GroupKind<R, P, G>, Child<Path, 2>>,
}

/// The complete bank, never a count, mask, sparse array or partial collection.
#[must_use]
pub struct VacantKeyBank<
    'id,
    R: InterfaceRole,
    const P: usize,
    const G: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    pairwise: [PairwiseKeySlot<'id, R, P, G, Path>; P],
    group: [GroupKeySlot<'id, R, P, G, Path>; G],
}

/// A collection being filled by resource returns. It is not a complete bank
/// and cannot end an association. Conversion retains all owners on failure.
pub struct KeyBankParts<P, G, const N: usize> {
    /// Vacant PTK slots received so far.
    pub pairwise: heapless::Vec<P, N>,
    /// Vacant GTK slots received so far.
    pub group: heapless::Vec<G, N>,
}

/// A start permission, the station's designated PTK slot and all spare slots.
pub type DistributedKeyBank<S, P, G, const N: usize> = (S, P, KeyBankParts<P, G, N>);

/// Every PTK and GTK slot of one association domain, in fixed-size arrays.
pub type KeyBankArrays<
    'id,
    R,
    const P: usize,
    const G: usize,
    Path = Root<oer_radio_port::LocalId<'id>>,
> = (
    [PairwiseKeySlot<'id, R, P, G, Path>; P],
    [GroupKeySlot<'id, R, P, G, Path>; G],
);

type BankParts<'id, R, const P: usize, const G: usize, Path, const N: usize> =
    KeyBankParts<PairwiseKeySlot<'id, R, P, G, Path>, GroupKeySlot<'id, R, P, G, Path>, N>;

impl<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    AssociationStart<'id, R, P, G, Path>
{
    /// Select once. This pure transition does not access a backend. A key
    /// installation must apply this decision before publishing key material.
    pub fn begin(self, spp: Spp) -> Association<'id, R, P, G, Path> {
        Association { start: self, spp }
    }
}
impl<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    Association<'id, R, P, G, Path>
{
    /// Decision fixed for this association.
    pub const fn spp(&self) -> Spp {
        self.spp
    }
}
impl<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantAssociation<'id, R, P, G, Path>
{
    /// Distribute the vacant slots while retaining the unique start permission.
    /// The slots alone cannot install a key: installation also requires the
    /// association produced by `AssociationStart::begin`.
    pub fn into_parts(
        self,
    ) -> (
        AssociationStart<'id, R, P, G, Path>,
        VacantKeyBank<'id, R, P, G, Path>,
    ) {
        (self.start, self.bank)
    }
}
impl<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantKeyBank<'id, R, P, G, Path>
{
    /// Assemble exactly every slot issued by this domain.
    pub fn new(
        pairwise: [PairwiseKeySlot<'id, R, P, G, Path>; P],
        group: [GroupKeySlot<'id, R, P, G, Path>; G],
    ) -> Self {
        Self { pairwise, group }
    }
    /// Return the complete slot arrays by value.
    pub fn into_parts(self) -> KeyBankArrays<'id, R, P, G, Path> {
        (self.pairwise, self.group)
    }
    /// Collect returns without losing an incomplete set. Only the success
    /// variant contains the complete, statically sized bank.
    pub fn collect<const N: usize>(
        parts: BankParts<'id, R, P, G, Path, N>,
    ) -> Result<Self, BankParts<'id, R, P, G, Path, N>> {
        let KeyBankParts { pairwise, group } = parts;
        let pairwise = match pairwise.into_array::<P>() {
            Ok(pairwise) => pairwise,
            Err(pairwise) => return Err(KeyBankParts { pairwise, group }),
        };
        match group.into_array::<G>() {
            Ok(group) => Ok(Self { pairwise, group }),
            Err(group) => Err(KeyBankParts {
                pairwise: heapless::Vec::from_array(pairwise),
                group,
            }),
        }
    }
    /// Distribute the bank to bounded protocol collections.
    pub fn into_collections<const N: usize>(self) -> BankParts<'id, R, P, G, Path, N> {
        KeyBankParts {
            pairwise: heapless::Vec::from_array(self.pairwise),
            group: heapless::Vec::from_array(self.group),
        }
    }
}
impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    PairwiseKeySlot<'_, R, P, G, Path>
{
    /// Position within this domain's slot bank.
    pub const fn index(&self) -> u8 {
        self.slot.index()
    }
}
impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    GroupKeySlot<'_, R, P, G, Path>
{
    /// Position within this domain's slot bank.
    pub const fn index(&self) -> u8 {
        self.slot.index()
    }
}

/// End after every installed key and admitted use has returned its vacant
/// slot. No hardware work remains: the bank proves the removal boundary.
/// The returned owner retains the bank for the next association.
pub fn end_association<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>(
    association: Association<'id, R, P, G, Path>,
    vacant: VacantKeyBank<'id, R, P, G, Path>,
) -> VacantAssociation<'id, R, P, G, Path> {
    let Association { start, .. } = association;
    VacantAssociation {
        start,
        bank: vacant,
    }
}

/// Reunite a distributed initial bank with its unused start permission.
/// This cannot bypass an active association: beginning consumed that permission.
pub fn reunite_vacant<'id, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>(
    start: AssociationStart<'id, R, P, G, Path>,
    bank: VacantKeyBank<'id, R, P, G, Path>,
) -> VacantAssociation<'id, R, P, G, Path> {
    VacantAssociation { start, bank }
}
