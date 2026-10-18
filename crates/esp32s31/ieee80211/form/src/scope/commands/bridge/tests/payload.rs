//! A model MPDU retains three actual pinned-pool DMA leases: header and two
//! bodies. Only its narrow execution view can project their address metadata.
use super::*;
use core::{convert::Infallible, marker::PhantomData};
use oer_ieee80211_lower_mac::*;
use oer_ieee80211_mac::address::IndividualAddress;
use std::boxed::Box;

type Lease = oer_memory::PinnedDmaTxRadioLease<'static, 32, 0, 0>;
pub(super) struct Mpdu {
    pub fragments: [Lease; MODEL_FRAGMENTS],
    pub sn: u16,
    pub pn: u64,
}
pub(super) struct Execution<'a> {
    fragments: &'a mut [Lease; MODEL_FRAGMENTS],
    sn: u16,
    pn: u64,
}
impl ExecutionPayload for Mpdu {
    type Execution<'a> = Execution<'a>;
    fn execution(&mut self) -> Execution<'_> {
        Execution {
            fragments: &mut self.fragments,
            sn: self.sn,
            pn: self.pn,
        }
    }
}
impl ProjectExecution for Mpdu {
    fn project(view: Execution<'_>) -> TxImage {
        use oer_memory::StableDmaBacking;
        let fragments = core::array::from_fn(|index| {
            let lease = &mut view.fragments[index];
            lease.prepare_for_dma_read();
            DmaSpan::from_region(lease.stable_dma_region())
        });
        TxImage {
            fragments,
            sequence: view.sn,
            packet_number: view.pn,
        }
    }
}
impl TxMpdu for Mpdu {
    type Buffer = Self;
    type Body = Infallible;
    fn into_parts(self) -> (Self, Option<Infallible>) {
        (self, None)
    }
}
pub(super) struct Prepared(pub Mpdu);
impl<'id> PreparedTx<'id> for Prepared {
    type Types = Types;
    type Execution = ();
    fn into_admission(self) -> (Mpdu, ()) {
        (self.0, ())
    }
    fn into_parts(self) -> (Mpdu, Option<Infallible>) {
        (self.0, None)
    }
}
pub(super) struct Types;
impl<'id> PhaseTypes<'id> for Types {
    const COMMAND_CAPACITY: usize = CommandPools::<2, 2>::PHASE_COMMAND_CAPACITY;
    const TERMINAL_CAPACITY: usize = Self::COMMAND_CAPACITY;
    type StationCommandPath = StationPath;
    type AccessPointCommandPath = AccessPointPath;
    type StationConfigureContext = usize;
    type AccessPointConfigureContext = usize;
    type EpochPath = Root;
    type GatePath = GatePath;
    type TicketPath = TicketPath;
    type BlockAckPath = Root;
    type Fault = Infallible;
    type Context = ();
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = Prepared;
    type KeyIdentity = ();
    type Aggregate<'k, 'p, R, K, S>
        = NoAggregate<S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k;
    type TxBody = Infallible;
    type TicketKind = TicketKind;
    type AckEvidence = ();
    type BlockAckEvidence = ();
    type AirDenied = Infallible;
    type AnyTxBuffer = Mpdu;
    type TxMpdu = Mpdu;
    type TxAggregate = NoAggregate<NonEmptyAggregate>;
    type RxBuffer = NoRx;
    type RxMeta = RxMeta;
    type StationKeys = NoKeys;
    type AccessPointKeys = NoKeys;
    type BlockAckKind = Infallible;
    type BlockAckPool = association::BlockAckSlots<Infallible, 0>;
    type BlockAck = Infallible;
}
pub(super) struct NoKeys;
impl<'id> KeyTypes<'id> for NoKeys {
    type Pool = association::AssociationPool<oer_ieee80211_lower_mac::Station, 0, 0>;
    type AssociationStart = Infallible;
    type Association = Infallible;
    type VacantKeyBank = Infallible;
    type VacantAssociation = Infallible;
    fn begin_association(start: Infallible, _: oer_ieee80211_mac::security::Spp) -> Infallible {
        match start {}
    }
    fn collect_key_bank<const N: usize>(
        parts: association::KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
    ) -> Result<Infallible, association::KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>> {
        Err(parts)
    }
    fn end_association(association: Infallible, _: Infallible) -> Infallible {
        match association {}
    }
    fn split_vacant<const N: usize>(
        vacant: Infallible,
    ) -> association::DistributedKeyBank<Infallible, Self::PairwiseSlot, Self::GroupSlot, N> {
        match vacant {}
    }

    type PairwiseSlot = association::PairwiseKeySlot<'id, oer_ieee80211_lower_mac::Station, 0, 0>;
    type GroupSlot = association::GroupKeySlot<'id, oer_ieee80211_lower_mac::Station, 0, 0>;
    type PairwiseKey = Infallible;
    type GroupKey = Infallible;
    type GroupPermission = Infallible;
    type Identity = ();
    type GroupSlotIdentity = ();
    fn pairwise_identity(key: &Infallible) {
        match *key {}
    }
    fn group_identity(key: &Infallible) {
        match *key {}
    }
    fn group_slot_identity(key: &Infallible) {
        match *key {}
    }
    fn vacant_group_slot_identity(_: &Self::GroupSlot) {}
}
pub(super) struct NoRx(Infallible);
impl RxBuffer for NoRx {
    fn bytes(&self) -> &[u8] {
        match self.0 {}
    }
}
pub(super) struct NoAggregate<S>(Infallible, PhantomData<S>);
impl<S> IntoIterator for NoAggregate<S> {
    type Item = Mpdu;
    type IntoIter = core::iter::Empty<Mpdu>;
    fn into_iter(self) -> Self::IntoIter {
        match self.0 {}
    }
}
impl<S> ExecutionPayload for NoAggregate<S> {
    type Execution<'a>
        = Infallible
    where
        S: 'a;
    fn execution(&mut self) -> Infallible {
        match self.0 {}
    }
}
impl<'id, S: AggregateState> PhaseAggregate<'id> for NoAggregate<S> {
    type Types = Types;
    type State = S;
    type Execution = Infallible;
    type Cancelled = core::iter::Empty<Mpdu>;
    fn into_admission(self) -> (NoAggregate<NonEmptyAggregate>, Infallible)
    where
        S: NonEmptyState,
    {
        match self.0 {}
    }
    fn receiver(&self) -> IndividualAddress {
        match self.0 {}
    }
    fn tid(&self) -> Tid {
        match self.0 {}
    }
    fn protection(&self) -> MpduProtection {
        match self.0 {}
    }
    fn subframes(&self) -> usize {
        match self.0 {}
    }
    fn into_parts(self) -> Self::Cancelled {
        match self.0 {}
    }
}

pub(super) fn admit_with_pool<'id, C>(
    context: C,
    ticket: Slot<'id, TicketKind, TicketPath>,
    ordinal: u8,
) -> (
    InFlight<C, Slot<'id, TicketKind, TicketPath>, Mpdu>,
    oer_memory::PinnedDmaTxPoolHandle<32, 0, 0, MODEL_FRAGMENTS>,
) {
    let pool = oer_memory::PinnedDmaTxPool::<32, 0, 0, MODEL_FRAGMENTS>::pin_static(Box::leak(
        Box::new(oer_memory::PinnedDmaTxPool::new()),
    ));
    let fragments = core::array::from_fn(|index| {
        pool.claim_network(index as u8)
            .publish(32, |bytes| bytes.fill(ordinal + index as u8));
        pool.claim_radio(index as u8)
    });
    let operation = Attempt {
        context,
        ticket,
        source: Prepared(Mpdu {
            fragments,
            sn: 10 + u16::from(ordinal),
            pn: 500 + u64::from(ordinal),
        }),
        access_category: Default::default(),
        protection: Protection::None,
        power: SupportedPower::<Caps>::new(TxPower::Calibrated).unwrap(),
        coex: SupportedCoex::<Caps>::new(CoexPriority::Normal).unwrap(),
    }
    .admit()
    .operation;
    (operation, pool)
}

pub(super) fn admit<'id, C>(
    context: C,
    ticket: Slot<'id, TicketKind, TicketPath>,
    ordinal: u8,
) -> InFlight<C, Slot<'id, TicketKind, TicketPath>, Mpdu> {
    admit_with_pool(context, ticket, ordinal).0
}
