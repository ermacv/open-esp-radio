//! One core and its four independently owned EDCA queue places.

use oer_esp32s31_ieee80211_mac::tx::LegacyTxQueue;
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_radio_port::{Child, PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots};

enum CorePlace {}
enum TicketPlace {}

/// Layout issued once within the backend's installation pools.
/// Tuple paths keep two cores in one installation distinct as well.
pub struct CorePools<const TICKETS: usize = 4>;

/// Construction authority and the queues of the one core it constructs.
pub struct CoreResources<'core, Path: PoolPath = Root, const TICKETS: usize = 4> {
    pub authority: CoreAuthority<'core, Path, TICKETS>,
    /// One admission credit for each TX ticket in the backend layout.
    pub tickets: [TxTicket<'core, Path>; TICKETS],
    pub station_association: super::association::VacantStationAssociation<'core, Path>,
    pub access_point_association: super::association::VacantAccessPointAssociation<'core, Path>,
    pub queues: [FreeQueue<'core, Path>; 4],
}

/// Unique authority to construct the core associated with these queue places.
pub struct CoreAuthority<'core, Path: PoolPath = Root, const TICKETS: usize = 4> {
    _place: Slot<'core, CorePlace, Child<Path, 0>>,
}

/// Permission for one attempt on one EDCA queue of one core.
///
/// Submission consumes this owner; its terminal or refusal returns it.
/// No API issues a second owner while an attempt occupies the queue.
/// Forgetting a place only removes capacity; it cannot permit an overwrite.
#[must_use = "retain the queue place until its next submission"]
pub struct FreeQueue<'core, Path: PoolPath = Root> {
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
                oer_ieee80211_lower_mac::association::AssociationPool<
                    oer_ieee80211_lower_mac::Station,
                    1,
                    1,
                >,
                oer_ieee80211_lower_mac::association::AssociationPool<
                    oer_ieee80211_lower_mac::AccessPoint,
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
pub struct TxTicket<'core, Path: PoolPath = Root> {
    place: Slot<'core, TicketPlace, Child<Path, 1>>,
}

impl<Path: PoolPath> TxTicket<'_, Path> {
    /// This ticket's installation-local position, for its owner's return bank.
    pub const fn index(&self) -> usize {
        self.place.index() as usize
    }
}
