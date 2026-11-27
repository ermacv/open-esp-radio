//! The two bundles of an installation's rights and the layout that names
//! them: the radio's, once, and one an interface. A layout is the radio's
//! long chains, a closed set ([`LongChains`]), and a tuple of one or two
//! interface layouts ([`InterfaceLayout`]), at most one a role
//! ([`RadioLayout`]); its mint gives the radio's bundle and the
//! interfaces'.

use core::marker::PhantomData;

use oer_portable_bounded::Index;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

use super::{
    AgreementSlot, CommandPlace, GroupSlot, LongChain, PairwiseSlot, QUEUES, Queue, ReceiveOff,
};
use crate::life::{MintSeal, sealed};
use crate::{InterfaceRole, VifRole};

/// The radio's rights, once an installation: its long chains of `SLOTS`
/// descriptors for aggregates, the set `S` names, four queues, each with
/// its chain of one descriptor, and the command's place. The machine holds
/// them while the data is stopped, the engine while it flows; a retirement
/// takes them.
#[must_use = "the radio's rights each have one holder"]
pub struct RadioRights<'id, S: LongChains, const SLOTS: usize> {
    /// The long chains of `SLOTS` descriptors, one a marker of the set.
    pub chains: S::Rights<'id, SLOTS>,
    /// The queues, in the order of [`QUEUES`], each with its chain of one.
    pub queues: [Queue<'id>; 4],
    /// The command's place.
    pub command: CommandPlace<'id>,
}

impl<S: LongChains, const SLOTS: usize> core::fmt::Debug for RadioRights<'_, S, SLOTS>
where
    for<'id> S::Rights<'id, SLOTS>: core::fmt::Debug,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RadioRights")
            .field("chains", &self.chains)
            .field("queues", &self.queues)
            .field("command", &self.command)
            .finish()
    }
}

/// The marker of a layout's first long chain.
#[derive(Debug)]
pub enum First {}
/// The marker of a layout's second long chain.
#[derive(Debug)]
pub enum Second {}

impl sealed::Sealed for First {}
impl sealed::Sealed for Second {}

/// The marker of one long chain of a set: its right carries it as a type,
/// as an interface's right carries its role. Only this crate implements it
/// ([`First`], [`Second`]).
pub trait LongMarker: sealed::Sealed + 'static {}
impl LongMarker for First {}
impl LongMarker for Second {}

/// The long chains of a layout: a closed set, none, one or two, each a
/// marker. Only this crate implements it: `()`, `(First,)` and
/// `(First, Second)`; a third is a measured need's.
pub trait LongChains: sealed::Sealed + 'static {
    /// The set's rights: a long chain a marker.
    type Rights<'id, const SLOTS: usize>;
    /// A port's storage of the set: one `T` a long chain, each reached by
    /// its marker ([`LongIn::buffer`]).
    type Store<T>;
    /// Build the rights, for the installation's mint alone.
    fn mint<'id, const SLOTS: usize>(seal: MintSeal) -> Self::Rights<'id, SLOTS>;
    /// A storage of the set, each `T` made by `fill`.
    fn store<T>(fill: impl FnMut() -> T) -> Self::Store<T>;
    /// Each long chain's `T` of a storage, in the markers' order.
    fn each<'s, T: 's>(store: &'s Self::Store<T>) -> impl Iterator<Item = &'s T>;
    /// The same, to change.
    fn each_mut<'s, T: 's>(store: &'s mut Self::Store<T>) -> impl Iterator<Item = &'s mut T>;
}

/// A marker of the set `S`: it reaches its long chain's `T` of a storage
/// of the set, with no index. Only this crate implements it.
pub trait LongIn<S: LongChains>: LongMarker {
    /// The marker's `T`.
    fn buffer<T>(store: &mut S::Store<T>) -> &mut T;
    /// The same, to read.
    fn buffer_ref<T>(store: &S::Store<T>) -> &T;
}

/// A long chain's right of the marker `M`, as the mint makes it.
const fn long<'id, M, const SLOTS: usize>(seal: MintSeal) -> LongChain<'id, M, SLOTS> {
    let MintSeal(()) = seal;
    LongChain {
        brand: PhantomData,
        marker: PhantomData,
    }
}

impl sealed::Sealed for () {}
impl sealed::Sealed for (First,) {}
impl sealed::Sealed for (First, Second) {}

impl LongChains for () {
    type Rights<'id, const SLOTS: usize> = ();
    type Store<T> = ();
    fn mint<'id, const SLOTS: usize>(seal: MintSeal) -> Self::Rights<'id, SLOTS> {
        let MintSeal(()) = seal;
    }
    fn store<T>(_fill: impl FnMut() -> T) -> Self::Store<T> {}
    fn each<'s, T: 's>(_store: &'s Self::Store<T>) -> impl Iterator<Item = &'s T> {
        core::iter::empty()
    }
    fn each_mut<'s, T: 's>(_store: &'s mut Self::Store<T>) -> impl Iterator<Item = &'s mut T> {
        core::iter::empty()
    }
}

impl LongChains for (First,) {
    type Rights<'id, const SLOTS: usize> = (LongChain<'id, First, SLOTS>,);
    type Store<T> = (T,);
    fn mint<'id, const SLOTS: usize>(seal: MintSeal) -> Self::Rights<'id, SLOTS> {
        (long(seal),)
    }
    fn store<T>(mut fill: impl FnMut() -> T) -> Self::Store<T> {
        (fill(),)
    }
    fn each<'s, T: 's>(store: &'s Self::Store<T>) -> impl Iterator<Item = &'s T> {
        core::iter::once(&store.0)
    }
    fn each_mut<'s, T: 's>(store: &'s mut Self::Store<T>) -> impl Iterator<Item = &'s mut T> {
        core::iter::once(&mut store.0)
    }
}

impl LongChains for (First, Second) {
    type Rights<'id, const SLOTS: usize> =
        (LongChain<'id, First, SLOTS>, LongChain<'id, Second, SLOTS>);
    type Store<T> = (T, T);
    fn mint<'id, const SLOTS: usize>(seal: MintSeal) -> Self::Rights<'id, SLOTS> {
        let MintSeal(()) = seal;
        (long(MintSeal(())), long(MintSeal(())))
    }
    fn store<T>(mut fill: impl FnMut() -> T) -> Self::Store<T> {
        (fill(), fill())
    }
    fn each<'s, T: 's>(store: &'s Self::Store<T>) -> impl Iterator<Item = &'s T> {
        [&store.0, &store.1].into_iter()
    }
    fn each_mut<'s, T: 's>(store: &'s mut Self::Store<T>) -> impl Iterator<Item = &'s mut T> {
        let (first, second) = store;
        [first, second].into_iter()
    }
}

impl LongIn<(First,)> for First {
    fn buffer<T>(store: &mut (T,)) -> &mut T {
        &mut store.0
    }
    fn buffer_ref<T>(store: &(T,)) -> &T {
        &store.0
    }
}

impl LongIn<(First, Second)> for First {
    fn buffer<T>(store: &mut (T, T)) -> &mut T {
        &mut store.0
    }
    fn buffer_ref<T>(store: &(T, T)) -> &T {
        &store.0
    }
}

impl LongIn<(First, Second)> for Second {
    fn buffer<T>(store: &mut (T, T)) -> &mut T {
        &mut store.1
    }
    fn buffer_ref<T>(store: &(T, T)) -> &T {
        &store.1
    }
}

/// A port's storage of an installation's chains, a `T` each: the long
/// chains' of the set `S`, reached by their markers, and the chains of one,
/// reached by their queues' categories. A chain reaches its own
/// ([`ChainOf::buffer`](crate::ChainOf::buffer)): there is no index to look
/// up and no answer that it is absent.
pub struct Buffers<T, S: LongChains> {
    long: S::Store<T>,
    ones: [T; 4],
}

impl<T, S: LongChains> Buffers<T, S> {
    /// A storage, each `T` made by `fill`.
    pub fn new(mut fill: impl FnMut() -> T) -> Self {
        Self {
            long: S::store(&mut fill),
            ones: [(); 4].map(|()| fill()),
        }
    }

    /// The long chains' `T`, in their markers' order.
    pub fn longs(&self) -> impl Iterator<Item = &T> {
        S::each(&self.long)
    }

    /// Every chain's `T`: the long ones, then the chains of one.
    pub fn each(&self) -> impl Iterator<Item = &T> {
        S::each(&self.long).chain(self.ones.iter())
    }

    /// The same, to change.
    pub fn each_mut(&mut self) -> impl Iterator<Item = &mut T> {
        S::each_mut(&mut self.long).chain(self.ones.iter_mut())
    }

    /// The long chains' `T`, to change.
    pub fn longs_mut(&mut self) -> impl Iterator<Item = &mut T> {
        S::each_mut(&mut self.long)
    }

    /// The chains of one's `T`, to change.
    pub fn ones_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.ones.iter_mut()
    }

    /// The `T` of the long chain of the marker `M`.
    pub(crate) fn long_mut<M: LongIn<S>>(&mut self) -> &mut T {
        M::buffer(&mut self.long)
    }

    /// The same, to read.
    pub(crate) fn long_ref<M: LongIn<S>>(&self) -> &T {
        M::buffer_ref(&self.long)
    }

    /// The `T` of the chain of one of the queue of `category`.
    pub(crate) const fn one_mut(&mut self, category: WmmAccessCategory) -> &mut T {
        category.select_mut(&mut self.ones)
    }

    /// The same, to read.
    pub(crate) const fn one_ref(&self, category: WmmAccessCategory) -> &T {
        category.select(&self.ones)
    }
}

/// The rights of one interface of the role `I`, all empty: `PAIRWISE`
/// pairwise key places, its group key slots, its receive agreements' slots
/// and its receive rule, off. Whole, it is the proof that the interface is
/// taken down in the hardware: its keys removed, its agreements ended, its
/// address and receive rule cleared. A retirement takes it.
#[must_use = "an interface's rights each have one holder"]
#[derive(Debug)]
pub struct InterfaceRights<
    'id,
    I: InterfaceRole,
    const PAIRWISE: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> {
    /// The pairwise key places.
    pub pairwise: [PairwiseSlot<'id, I>; PAIRWISE],
    /// The group key slots.
    pub groups: [GroupSlot<'id, I>; GROUPS],
    /// The receive agreements' slots.
    pub agreements: [AgreementSlot<'id, I>; AGREEMENTS],
    /// The receive rule, off.
    pub receive: ReceiveOff<'id, I>,
}

/// The layout of one interface: its role `I` and its counts.
pub struct InterfaceLayout<I, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize>(
    PhantomData<fn() -> I>,
);

/// One member of a layout: an interface's layout. Only this crate
/// implements it ([`InterfaceLayout`]).
pub trait Member: sealed::Sealed {
    /// The interface's role.
    type Role: InterfaceRole;
    /// The interface's rights.
    type Rights<'id>;
    /// Build them, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
}

impl<I, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize> sealed::Sealed
    for InterfaceLayout<I, PAIRWISE, GROUPS, AGREEMENTS>
{
}

impl<I, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize>
    InterfaceLayout<I, PAIRWISE, GROUPS, AGREEMENTS>
{
    /// An interface's places are counted in a byte: evaluated by its mint,
    /// a layout of more does not build.
    pub const CHECKED: () = if PAIRWISE > u8::MAX as usize
        || GROUPS > u8::MAX as usize
        || AGREEMENTS > u8::MAX as usize
    {
        panic!("an interface's places are counted in a byte")
    };
}

impl<I: InterfaceRole, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize> Member
    for InterfaceLayout<I, PAIRWISE, GROUPS, AGREEMENTS>
{
    type Role = I;
    type Rights<'id> = InterfaceRights<'id, I, PAIRWISE, GROUPS, AGREEMENTS>;

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        let MintSeal(()) = seal;
        InterfaceRights {
            pairwise: Index::<PAIRWISE>::every().map(|index| PairwiseSlot {
                index: index.to_u8(),
                brand: PhantomData,
                interface: PhantomData,
            }),
            groups: Index::<GROUPS>::every().map(|index| GroupSlot {
                index: index.to_u8(),
                brand: PhantomData,
                interface: PhantomData,
            }),
            agreements: Index::<AGREEMENTS>::every().map(|index| AgreementSlot {
                index: index.to_u8(),
                brand: PhantomData,
                interface: PhantomData,
            }),
            receive: ReceiveOff {
                brand: PhantomData,
                interface: PhantomData,
            },
        }
    }
}

/// The interfaces of an installation: a closed set, a tuple of one or two
/// interface layouts with at most one of a role, which its mint checks at
/// build. Only this crate implements it.
pub trait Interfaces: sealed::Sealed {
    /// The interfaces' rights: one bundle for one interface, a tuple of
    /// them for two, branded with the installation.
    type Rights<'id>;
    /// The one check of the set, evaluated where it is named (its mint):
    /// its members are of distinct roles. A set with two interfaces of one
    /// role does not build.
    const CHECKED: ();
    /// Build the rights, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
}

/// The layout of an installation: the radio's long chains `S` of `SLOTS`
/// descriptors and its interfaces `M`.
pub struct RadioLayout<S, const SLOTS: usize, M>(PhantomData<fn() -> (S, M)>);

/// The layout of an installation ([`RadioLayout`]). Only this crate
/// implements it, and only its installation's mint calls [`Self::mint`]:
/// it takes a seal no other crate can make. A port serves the layouts its
/// `RadioPort` is implemented for.
pub trait Layout: sealed::Sealed {
    /// The rights: the radio's, and its interfaces' (one bundle for one
    /// interface, a tuple of them for more), branded with the installation.
    type Rights<'id>;
    /// The long chains of the layout.
    type Longs: LongChains;
    /// The checks of the layout, evaluated where it is named (its mint):
    /// its interfaces' ([`Interfaces::CHECKED`]), and a long chain's
    /// descriptors from one to a frame set's 64.
    const CHECKED: ();
    /// Build the rights, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
}

impl<S: LongChains, const SLOTS: usize, M: Interfaces> sealed::Sealed for RadioLayout<S, SLOTS, M> {}

impl<S: LongChains, const SLOTS: usize, M: Interfaces> Layout for RadioLayout<S, SLOTS, M> {
    type Rights<'id> = (RadioRights<'id, S, SLOTS>, M::Rights<'id>);
    type Longs = S;
    const CHECKED: () = {
        let () = M::CHECKED;
        if SLOTS == 0 || SLOTS > 64 {
            panic!("a long chain has one to 64 descriptors")
        }
    };

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        let MintSeal(()) = seal;
        (radio(MintSeal(())), M::mint(MintSeal(())))
    }
}

/// The rights of an installation of the layout `L`.
pub type LayoutRights<'id, L> = <L as Layout>::Rights<'id>;

/// The radio's rights, as the mint makes them.
fn radio<'id, S: LongChains, const SLOTS: usize>(seal: MintSeal) -> RadioRights<'id, S, SLOTS> {
    let MintSeal(()) = seal;
    RadioRights {
        chains: S::mint(MintSeal(())),
        queues: QUEUES.map(|category| Queue {
            category,
            brand: PhantomData,
        }),
        command: CommandPlace { brand: PhantomData },
    }
}

/// Two members are of one role: no layout has two interfaces of a role.
/// Every pair of roles is named, so a new role does not build until its
/// pairs are.
const fn one_role(first: VifRole, second: VifRole) -> bool {
    match (first, second) {
        (VifRole::Station, VifRole::Station) | (VifRole::AccessPoint, VifRole::AccessPoint) => true,
        (VifRole::Station, VifRole::AccessPoint) | (VifRole::AccessPoint, VifRole::Station) => {
            false
        }
    }
}

impl<A: Member> sealed::Sealed for (A,) {}
impl<A: Member, B: Member> sealed::Sealed for (A, B) {}

impl<A: Member> Interfaces for (A,) {
    type Rights<'id> = A::Rights<'id>;
    const CHECKED: () = ();

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        A::mint(seal)
    }
}

impl<A: Member, B: Member> Interfaces for (A, B) {
    type Rights<'id> = (A::Rights<'id>, B::Rights<'id>);
    const CHECKED: () = if one_role(A::Role::ROLE, B::Role::ROLE) {
        panic!("a layout has at most one interface of a role")
    };

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        let MintSeal(()) = seal;
        (A::mint(MintSeal(())), B::mint(MintSeal(())))
    }
}
