//! The two bundles of an installation's rights and the layout that names
//! them: the radio's, once, and one an interface. A layout is the radio's
//! chains and a tuple of one or two interface layouts ([`InterfaceLayout`]),
//! at most one a role ([`RadioLayout`]); its mint gives the radio's bundle
//! and the interfaces'.

use core::marker::PhantomData;

use oer_portable_bounded::Index;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

use super::{
    AgreementSlot, CommandPlace, GroupSlot, IdleChain, PairwiseSlot, QUEUES, QueueRight,
    ReceiveOff, SinglePlace,
};
use crate::life::{MintSeal, sealed};
use crate::{AccessPoint, InterfaceRole, Station, VifRole};

/// The radio's rights, once an installation: `LONG` chains of `SLOTS`
/// descriptors for aggregates, a chain of one descriptor a transmit queue,
/// a right a queue and the command's place. The machine holds them while
/// the data is stopped, the engine while it flows; a retirement takes them.
#[must_use = "the radio's rights each have one holder"]
#[derive(Debug)]
pub struct RadioRights<'id, const LONG: usize, const SLOTS: usize> {
    /// The chains of `SLOTS` descriptors: an aggregate's.
    pub chains: [IdleChain<'id, SLOTS>; LONG],
    /// The chains of one descriptor, one a queue: a free queue always finds
    /// one.
    pub ones: [IdleChain<'id, 1>; QUEUES.len()],
    /// A right a queue, in the order of [`QUEUES`].
    pub queues: [QueueRight<'id>; 4],
    /// The command's place.
    pub command: CommandPlace<'id>,
}

/// The rights of one interface of the role `I`, all empty: its single
/// places, its role's data places, `PAIRWISE` pairwise key places, its
/// group key slots, its receive agreements' slots and its receive rule,
/// off. Whole, it is the proof that the interface is taken down in the
/// hardware: its singles home, its keys removed, its agreements ended, its
/// address and receive rule cleared. A retirement takes it.
#[must_use = "an interface's rights each have one holder"]
#[derive(Debug)]
pub struct InterfaceRights<
    'id,
    I: InterfaceRole,
    const SINGLES: usize,
    const PAIRWISE: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> {
    /// The controller's single places, one a purpose.
    pub singles: [SinglePlace<'id, I>; SINGLES],
    /// The role's data places: none for a station, whose data goes in
    /// the arenas; one a transmit queue for an access point.
    pub data: I::Data<'id>,
    /// The pairwise key places.
    pub pairwise: [PairwiseSlot<'id, I>; PAIRWISE],
    /// The group key slots.
    pub groups: [GroupSlot<'id, I>; GROUPS],
    /// The receive agreements' slots.
    pub agreements: [AgreementSlot<'id, I>; AGREEMENTS],
    /// The receive rule, off.
    pub receive: ReceiveOff<'id, I>,
}

/// An access point's data places: one a transmit queue, in the order of
/// [`QUEUES`], indexed after its controller's single places.
pub type AccessPointData<'id> = [SinglePlace<'id, AccessPoint>; QUEUES.len()];

/// The layout of one interface: its role `I` and its counts.
pub struct InterfaceLayout<
    I,
    const SINGLES: usize,
    const PAIRWISE: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
>(PhantomData<fn() -> I>);

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

impl<I, const SINGLES: usize, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize>
    sealed::Sealed for InterfaceLayout<I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>
{
}

impl<I, const SINGLES: usize, const PAIRWISE: usize, const GROUPS: usize, const AGREEMENTS: usize>
    InterfaceLayout<I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>
{
    /// An interface's places are counted in a byte, its data places (a
    /// queue's count at most) after its single places: evaluated by its
    /// mint, a layout of more does not build.
    pub const CHECKED: () = if SINGLES + QUEUES.len() > u8::MAX as usize
        || PAIRWISE > u8::MAX as usize
        || GROUPS > u8::MAX as usize
        || AGREEMENTS > u8::MAX as usize
    {
        panic!("an interface's places are counted in a byte")
    };
}

impl<
    I: InterfaceRole,
    const SINGLES: usize,
    const PAIRWISE: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Member for InterfaceLayout<I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>
{
    type Role = I;
    type Rights<'id> = InterfaceRights<'id, I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>;

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        InterfaceRights {
            singles: Index::<SINGLES>::every().map(|index| SinglePlace {
                index: index.to_u8(),
                brand: PhantomData,
                interface: PhantomData,
            }),
            // After the controller's: each place's index is the interface's
            // own. The const check above keeps it in a byte.
            data: I::data(seal, Index::<256>::constant::<SINGLES>().to_u8()),
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

/// A station's data places: none, its data goes in the arenas.
pub(crate) const fn station_data<'id>(
    seal: MintSeal,
    _first: u8,
) -> <Station as InterfaceRole>::Data<'id> {
    let MintSeal(()) = seal;
}

/// An access point's data places: one a queue, indexed from `first`.
pub(crate) fn access_point_data<'id>(seal: MintSeal, first: u8) -> AccessPointData<'id> {
    let MintSeal(()) = seal;
    Index::<{ QUEUES.len() }>::every().map(|index| SinglePlace {
        // `first` is the interface's single places' count, which its
        // layout's const check keeps a queue's count below a byte's end.
        index: first.saturating_add(index.to_u8()),
        brand: PhantomData,
        interface: PhantomData,
    })
}

/// A single the port published, as it keeps it until its end: the
/// interface it was published as, its place's index and its queue. Made
/// only by the port's key ([`PortKey::single_published`](crate::PortKey::single_published))
/// and taken by its end ([`PortKey::single_end`](crate::PortKey::single_end)):
/// there is one of it a single on the air.
#[must_use = "a published single's end gives its place and its queue back"]
#[derive(Debug, Eq, PartialEq)]
pub struct PublishedSingle {
    role: VifRole,
    index: u8,
    category: WmmAccessCategory,
}

impl PublishedSingle {
    pub(crate) const fn new(role: VifRole, index: u8, category: WmmAccessCategory) -> Self {
        Self {
            role,
            index,
            category,
        }
    }

    /// The interface's role: the hardware's selector the port reads.
    pub const fn role(&self) -> VifRole {
        self.role
    }

    /// The place's index, its interface's own.
    pub const fn index(&self) -> u8 {
        self.index
    }

    /// The queue it went on.
    pub const fn category(&self) -> WmmAccessCategory {
        self.category
    }
}

/// A single's place of a layout of two interfaces, as its interface's.
#[derive(Debug)]
pub enum SingleOfTwo<'id, A: InterfaceRole, B: InterfaceRole> {
    /// The first interface's.
    First(SinglePlace<'id, A>),
    /// The second interface's.
    Second(SinglePlace<'id, B>),
}

/// The interfaces of an installation: a closed set, a tuple of one or two
/// interface layouts with at most one of a role, which its mint checks at
/// build. Only this crate implements it.
pub trait Interfaces: sealed::Sealed {
    /// The interfaces' rights: one bundle for one interface, a tuple of
    /// them for two, branded with the installation.
    type Rights<'id>;
    /// A single's place at its end, as the interface it was published as.
    type Single<'id>;
    /// The one check of the set, evaluated where it is named (its mint):
    /// its members are of distinct roles. A set with two interfaces of one
    /// role does not build.
    const CHECKED: ();
    /// Build the rights, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
    /// The place of a single the port published, made again at its end.
    fn single<'id>(seal: MintSeal, published: &PublishedSingle) -> Self::Single<'id>;
}

/// The layout of an installation: the radio's `LONG` chains of `SLOTS`
/// descriptors and its interfaces `M`.
pub struct RadioLayout<const LONG: usize, const SLOTS: usize, M>(PhantomData<fn() -> M>);

/// The layout of an installation ([`RadioLayout`]). Only this crate
/// implements it, and only its installation's mint calls [`Self::mint`]:
/// it takes a seal no other crate can make. A port serves the layouts its
/// `RadioPort` is implemented for.
pub trait Layout: sealed::Sealed {
    /// The rights: the radio's, and its interfaces' (one bundle for one
    /// interface, a tuple of them for more), branded with the installation.
    type Rights<'id>;
    /// A single's place at its end, as the interface it was published as.
    type Single<'id>;
    /// The checks of the layout, evaluated where it is named (its mint):
    /// its interfaces' ([`Interfaces::CHECKED`]), and a chain's
    /// descriptors from one to a frame set's 64, its chains counted in a
    /// byte.
    const CHECKED: ();
    /// Build the rights, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
    /// The place of a single the port published, made again at its end.
    fn single<'id>(seal: MintSeal, published: &PublishedSingle) -> Self::Single<'id>;
}

impl<const LONG: usize, const SLOTS: usize, M: Interfaces> sealed::Sealed
    for RadioLayout<LONG, SLOTS, M>
{
}

impl<const LONG: usize, const SLOTS: usize, M: Interfaces> Layout for RadioLayout<LONG, SLOTS, M> {
    type Rights<'id> = (RadioRights<'id, LONG, SLOTS>, M::Rights<'id>);
    type Single<'id> = M::Single<'id>;
    const CHECKED: () = {
        let () = M::CHECKED;
        if SLOTS == 0 || SLOTS > 64 || LONG.saturating_add(QUEUES.len()) > 1 << 8 {
            panic!(
                "a chain has one to 64 descriptors, and an installation's chains, the long ones and one a queue, are counted in a byte"
            )
        }
    };

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        let MintSeal(()) = seal;
        (radio(MintSeal(())), M::mint(MintSeal(())))
    }

    fn single<'id>(seal: MintSeal, published: &PublishedSingle) -> Self::Single<'id> {
        M::single(seal, published)
    }
}

/// The rights of an installation of the layout `L`.
pub type LayoutRights<'id, L> = <L as Layout>::Rights<'id>;

/// The radio's rights, as the mint makes them.
fn radio<'id, const LONG: usize, const SLOTS: usize>(
    seal: MintSeal,
) -> RadioRights<'id, LONG, SLOTS> {
    let MintSeal(()) = seal;
    RadioRights {
        chains: Index::<LONG>::every().map(|index| IdleChain {
            index: index.to_u8(),
            brand: PhantomData,
        }),
        // The radio's chains are numbered once: the long ones, then the
        // chains of one from `LONG`. `CHECKED` holds every number in a
        // byte, so the sum neither saturates nor leaves its residue.
        ones: Index::<{ QUEUES.len() }>::every().map(|index| IdleChain {
            index: Index::<{ 1 << 8 }>::residue(LONG.saturating_add(index.get())).to_u8(),
            brand: PhantomData,
        }),
        queues: QUEUES.map(|category| QueueRight {
            category,
            brand: PhantomData,
        }),
        command: CommandPlace { brand: PhantomData },
    }
}

/// The place `published` names, as the interface `I`'s.
const fn place<'id, I>(seal: MintSeal, published: &PublishedSingle) -> SinglePlace<'id, I> {
    let MintSeal(()) = seal;
    SinglePlace {
        index: published.index,
        brand: PhantomData,
        interface: PhantomData,
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
    type Single<'id> = SinglePlace<'id, A::Role>;
    const CHECKED: () = ();

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        A::mint(seal)
    }

    /// The layout's one interface: every single it publishes is its.
    fn single<'id>(seal: MintSeal, published: &PublishedSingle) -> Self::Single<'id> {
        place(seal, published)
    }
}

impl<A: Member, B: Member> Interfaces for (A, B) {
    type Rights<'id> = (A::Rights<'id>, B::Rights<'id>);
    type Single<'id> = SingleOfTwo<'id, A::Role, B::Role>;
    const CHECKED: () = if one_role(A::Role::ROLE, B::Role::ROLE) {
        panic!("a layout has at most one interface of a role")
    };

    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id> {
        const { Self::CHECKED };
        let MintSeal(()) = seal;
        (A::mint(MintSeal(())), B::mint(MintSeal(())))
    }

    /// The two members are of two roles (the mint's check), so the role
    /// the single was published as names its member.
    fn single<'id>(seal: MintSeal, published: &PublishedSingle) -> Self::Single<'id> {
        if one_role(published.role, A::Role::ROLE) {
            SingleOfTwo::First(place(seal, published))
        } else {
            SingleOfTwo::Second(place(seal, published))
        }
    }
}
