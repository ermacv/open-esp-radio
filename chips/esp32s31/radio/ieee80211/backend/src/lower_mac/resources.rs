//! One core and its four independently owned EDCA queue places.

use crate::mac::tx::LegacyTxQueue;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_radio_port::{
    Child, PoolBuilder, PoolPath, PoolSet, Root, Slot, SlotBank, Slots,
};

enum CorePlace {}
enum TicketPlace {}

/// Layout issued once within the backend's installation pools.
/// Tuple paths keep two cores in one installation distinct as well.
pub struct CorePools<const TICKETS: usize = 4>;

/// Construction authority and the queues of the one core it constructs.
pub struct CoreResources<
    'core,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>,
    const TICKETS: usize = 4,
> {
    pub authority: CoreAuthority<'core, Path, TICKETS>,
    /// One admission credit for each TX ticket in the backend layout.
    pub tickets: [TxTicket<'core, Path>; TICKETS],
    /// The association domains, which the installation issues with its
    /// generation.
    pub station_association: super::association::StationAssociationGrant<'core, Path>,
    pub access_point_association: super::association::AccessPointAssociationGrant<'core, Path>,
    pub queues: [FreeQueue<'core, Path>; 4],
}

/// Unique authority to construct the core associated with these queue places.
pub struct CoreAuthority<
    'core,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>,
    const TICKETS: usize = 4,
> {
    _place: Slot<'core, CorePlace, Child<Path, 0>>,
}

/// Permission for one attempt on one EDCA queue of one core.
///
/// Submission consumes this owner; its terminal or refusal returns it.
/// No API issues a second owner while an attempt occupies the queue.
/// Forgetting a place only removes capacity; it cannot permit an overwrite.
#[must_use = "retain the queue place until its next submission"]
pub struct FreeQueue<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    _place: Slot<'core, CorePlace, Child<Path, 0>>,
    queue: LegacyTxQueue,
}

impl<Path: PoolPath> FreeQueue<'_, Path> {
    /// The access category this unique place admits.
    pub const fn category(&self) -> WmmAccessCategory {
        self.queue.access_category()
    }
}

impl<const TICKETS: usize> PoolSet for CorePools<TICKETS> {
    type Pools<'core, Path: PoolPath> = CoreResources<'core, Path, TICKETS>;

    fn build<'core, Path: PoolPath>(builder: PoolBuilder<'core, Path>) -> Self::Pools<'core, Path> {
        const {
            assert!(TICKETS <= 256, "ticket indices fit the pool index");
            let () = super::keys::KeyUseCount::<TICKETS>::WIDTH_CHECK;
        }
        let (places, tickets, station_association, access_point_association) = builder
            .into_pools::<(
                Slots<CorePlace, 5>,
                Slots<TicketPlace, TICKETS>,
                oer_protocols_ieee80211_contracts_port::association::AssociationPool<
                    oer_protocols_ieee80211_contracts_port::Station,
                    1,
                    1,
                >,
                oer_protocols_ieee80211_contracts_port::association::AssociationPool<
                    oer_protocols_ieee80211_contracts_port::AccessPoint,
                    15,
                    3,
                >,
            )>();
        let [authority, voice, video, best_effort, background] = places;
        let place = |slot, queue| FreeQueue {
            _place: slot,
            queue,
        };
        CoreResources {
            station_association,
            access_point_association,
            authority: CoreAuthority { _place: authority },
            tickets: tickets.map(|place| TxTicket { place }),
            queues: [
                place(voice, LegacyTxQueue::Voice),
                place(video, LegacyTxQueue::Video),
                place(best_effort, LegacyTxQueue::BestEffort),
                place(background, LegacyTxQueue::Background),
            ],
        }
    }
}

/// One TX admission of this installation, returned at its terminal.
///
/// `CorePools<N>` issues exactly `N` tickets alongside its authority. Both
/// open and protected attempts consume one; a `KeyUse` contains its ticket,
/// so the number of key uses cannot exceed that same compile-time bound.
#[must_use = "the ticket is the capacity for one attempt"]
pub struct TxTicket<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    place: Slot<'core, TicketPlace, Child<Path, 1>>,
}

impl<Path: PoolPath> TxTicket<'_, Path> {
    /// This ticket's installation-local position, for its owner's return bank.
    pub const fn index(&self) -> usize {
        self.place.index() as usize
    }
}

/// The installation's tickets at home: a ticket returns to its own place
/// ([`SlotBank`]), at its terminal or at its refusal, never elsewhere.
pub struct TicketBank<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    places: SlotBank<'core, TicketPlace, Child<Path, 1>>,
}

impl<'core, Path: PoolPath> TicketBank<'core, Path> {
    /// A bank of no tickets.
    pub const fn empty() -> Self {
        Self {
            places: SlotBank::empty(),
        }
    }

    /// A bank holding `tickets` at their own places.
    pub fn new<const N: usize>(tickets: [TxTicket<'core, Path>; N]) -> Self {
        Self {
            places: SlotBank::new(tickets.map(|ticket| ticket.place)),
        }
    }

    /// Return a ticket to its own place.
    pub fn put(&mut self, ticket: TxTicket<'core, Path>) {
        self.places.put(ticket.place);
    }

    /// The ticket of the lowest index at home.
    pub fn take(&mut self) -> Option<TxTicket<'core, Path>> {
        self.places.take_first().map(|place| TxTicket { place })
    }

    /// How many tickets are at home.
    pub fn at_home(&self) -> u32 {
        self.places.at_home()
    }

    /// Whether every ticket is at home.
    pub fn all_home(&self) -> bool {
        self.places.all_home()
    }
}

/// The four EDCA queue places at home: a place returns to its own queue's
/// place at its completion or its refusal ([`SlotBank`]).
pub struct QueueBank<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    places: SlotBank<'core, CorePlace, Child<Path, 0>>,
}

/// The index the core's pools issue a queue's place at, after the
/// authority's.
const fn queue_place(queue: LegacyTxQueue) -> u8 {
    match queue {
        LegacyTxQueue::Voice => 1,
        LegacyTxQueue::Video => 2,
        LegacyTxQueue::BestEffort => 3,
        LegacyTxQueue::Background => 4,
    }
}

impl<'core, Path: PoolPath> QueueBank<'core, Path> {
    /// A bank of no queue places.
    pub const fn empty() -> Self {
        Self {
            places: SlotBank::empty(),
        }
    }

    /// A bank holding the four queue places at their own places.
    pub fn new(queues: [FreeQueue<'core, Path>; 4]) -> Self {
        Self {
            places: SlotBank::new(queues.map(|queue| queue._place)),
        }
    }

    /// Return a queue place to its own place.
    pub fn put(&mut self, queue: FreeQueue<'core, Path>) {
        self.places.put(queue._place);
    }

    /// The place of the queue of `category`, when it is at home.
    pub fn take(&mut self, category: WmmAccessCategory) -> Option<FreeQueue<'core, Path>> {
        let queue = LegacyTxQueue::from_access_category(category);
        self.places.take(queue_place(queue)).map(|place| FreeQueue {
            _place: place,
            queue,
        })
    }

    /// Whether the place of the queue of `category` is at home.
    pub fn is_home(&self, category: WmmAccessCategory) -> bool {
        self.places
            .is_home(queue_place(LegacyTxQueue::from_access_category(category)))
    }

    /// Whether the place of the queue of hardware index `index` is at
    /// home; an index of no queue has none.
    pub fn is_home_at(&self, index: usize) -> bool {
        [
            LegacyTxQueue::Voice,
            LegacyTxQueue::Video,
            LegacyTxQueue::BestEffort,
            LegacyTxQueue::Background,
        ]
        .into_iter()
        .any(|queue| {
            usize::from(queue.hardware_index()) == index && self.places.is_home(queue_place(queue))
        })
    }

    /// Whether every queue place is at home.
    pub fn all_home(&self) -> bool {
        self.places.all_home()
    }
}
