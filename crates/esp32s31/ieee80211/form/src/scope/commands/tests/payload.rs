//! Admission fixture using the public Attempt -> InFlight boundary. It owns
//! a header and two body allocations; no fake InFlight constructor is exposed.
use super::*;
use core::{convert::Infallible, marker::PhantomData};
use oer_ieee80211_lower_mac::*;
use oer_ieee80211_mac::address::IndividualAddress;
use std::boxed::Box;

pub(super) struct Mpdu {
    pub header: [u8; 24],
    pub bodies: [Box<[u8]>; 2],
    pub sn: u16,
    pub pn: u64,
}
pub(super) struct Execution<'a> {
    pub header: &'a [u8],
    pub bodies: [&'a [u8]; 2],
    pub sn: u16,
    pub pn: u64,
}
impl ExecutionPayload for Mpdu {
    type Execution<'a> = Execution<'a>;
    fn execution(&mut self) -> Execution<'_> {
        Execution {
            header: &self.header,
            bodies: [&self.bodies[0], &self.bodies[1]],
            sn: self.sn,
            pn: self.pn,
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

pub(super) fn admit<'id, 'context>(
    context: &'context str,
    ticket: Slot<'id, TicketKind, TicketPath>,
    ordinal: u8,
) -> InFlight<&'context str, Slot<'id, TicketKind, TicketPath>, Mpdu> {
    Attempt {
        context,
        ticket,
        source: Prepared(Mpdu {
            header: [ordinal; 24],
            bodies: [
                std::vec![ordinal; 7].into_boxed_slice(),
                std::vec![ordinal; 11].into_boxed_slice(),
            ],
            sn: 10 + u16::from(ordinal),
            pn: 500 + u64::from(ordinal),
        }),
        access_category: Default::default(),
        protection: Protection::None,
        power: SupportedPower::<Caps>::new(TxPower::Calibrated).unwrap(),
        coex: SupportedCoex::<Caps>::new(CoexPriority::Normal).unwrap(),
    }
    .admit()
    .operation
}
