//! The station's and its data machine's tokens on a lower-MAC port: tickets,
//! buffers of two classes, key slots and the installed pairwise key, as the
//! port's phase types `Ty` define them.

use core::marker::PhantomData;
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_port::association::{KEY_BANK_CAPACITY, KeyPool};
use oer_ieee80211_port::{
    InterfaceKeys, InterfacePhase, KeyTypes, NumberedKey, PhaseTypes, TxBufferClass, TxBufferPool,
};
use oer_ieee80211_upper_mac::ccmp::{
    CcmpPacketNumberStep, InstalledCcmpTxKey, install_ccmp_pairwise_key,
};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_radio_port::Slot;

use crate::data::StaDataTokens;
use crate::machine::StaTokens;

/// A marker of the install `'id` and two types.
type Marker<'id, A, B> = PhantomData<fn(&'id ()) -> (A, B)>;

/// The station's tokens on a port of the phase types `Ty`, its frames in
/// buffers of the class `S`.
pub struct PortStaTokens<'id, Ty, S, const GROUPS: usize = 2>(Marker<'id, Ty, S>);

/// The station PTK retains the unique association owner until its removal terminal.
pub struct StationPairwiseKey<'id, K: KeyTypes<'id>> {
    key: InstalledCcmpTxKey<K::PairwiseKey>,
    association: K::Association,
}

impl<'id, K: KeyTypes<'id>> StationPairwiseKey<'id, K> {
    /// Install the station's PTK for `peer` on the phase's station interface,
    /// numbering its MPDUs by `step`.
    pub fn install<'p, P>(
        phase: &mut P,
        association: K::Association,
        slot: <InterfaceKeys<'p, 'id, P> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
        step: CcmpPacketNumberStep,
    ) -> Self
    where
        'id: 'p,
        P: InterfacePhase<'p, 'id>,
        InterfaceKeys<'p, 'id, P>:
            KeyTypes<'id, Association = K::Association, PairwiseKey = K::PairwiseKey>,
    {
        Self {
            key: install_ccmp_pairwise_key(phase, &association, slot, peer, key, step),
            association,
        }
    }

    /// Number one protected MPDU: the port key and the key's next CCMP
    /// packet number, or `None` once its packet numbers are exhausted. A
    /// retransmission keeps the number of its first transmission.
    pub fn number(&self) -> Option<NumberedKey<&K::PairwiseKey>> {
        self.key.number()
    }

    /// The association owner and the port key, for the key's removal.
    pub fn into_parts(self) -> (K::Association, K::PairwiseKey) {
        (self.association, self.key.into_key())
    }
}

impl<'id, Ty, S, const GROUPS: usize> StaTokens for PortStaTokens<'id, Ty, S, GROUPS>
where
    Ty: PhaseTypes<'id, TxBody = core::convert::Infallible> + TxBufferPool<'id, S>,
    S: TxBufferClass,
{
    const PAIRWISE_SLOTS: usize =
        <<Ty::StationKeys as KeyTypes<'id>>::Pool as KeyPool>::PAIRWISE_SLOTS;
    const TOTAL_GROUP_SLOTS: usize =
        <<Ty::StationKeys as KeyTypes<'id>>::Pool as KeyPool>::GROUP_SLOTS;
    const GROUP_SLOTS: usize = GROUPS;
    type Small = S;
    type EpochPath = Ty::EpochPath;
    type ConfigurationPath = Ty::StationCommandPath;
    type GatePath = Ty::GatePath;
    type Ticket = Slot<'id, Ty::TicketKind, Ty::TicketPath>;
    type SmallBuffer = <Ty as TxBufferPool<'id, S>>::Buffer;
    type Sent = <Ty as TxBufferPool<'id, S>>::Sent;
    type AckEvidence = Ty::AckEvidence;
    type AirDenied = Ty::AirDenied;

    fn renew(mpdu: Self::Sent) -> Self::SmallBuffer {
        let (memory, body) = <Ty as TxBufferPool<'id, S>>::renew(mpdu);
        if let Some(body) = body {
            match body {}
        }
        memory
    }
    type PairwiseSlot = <Ty::StationKeys as KeyTypes<'id>>::PairwiseSlot;
    type GroupSlot = <Ty::StationKeys as KeyTypes<'id>>::GroupSlot;
    type PairwiseKey = StationPairwiseKey<'id, Ty::StationKeys>;
    type AssociationStart = <Ty::StationKeys as KeyTypes<'id>>::AssociationStart;
    type Association = <Ty::StationKeys as KeyTypes<'id>>::Association;
    type VacantKeyBank = <Ty::StationKeys as KeyTypes<'id>>::VacantKeyBank;
    type VacantAssociation = <Ty::StationKeys as KeyTypes<'id>>::VacantAssociation;
    fn begin_association(
        start: Self::AssociationStart,
        spp: oer_ieee80211_wire::security::Spp,
    ) -> Self::Association {
        Ty::StationKeys::begin_association(start, spp)
    }
    fn collect_key_bank(
        parts: oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    ) -> Result<
        Self::VacantKeyBank,
        oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    > {
        Ty::StationKeys::collect_key_bank(parts)
    }
    fn split_vacant(
        vacant: Self::VacantAssociation,
    ) -> (
        Self::AssociationStart,
        Self::PairwiseSlot,
        oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    ) {
        Ty::StationKeys::split_vacant(vacant)
    }
    fn end_association(
        association: Self::Association,
        bank: Self::VacantKeyBank,
    ) -> (
        Self::AssociationStart,
        Self::PairwiseSlot,
        oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    ) {
        Ty::StationKeys::split_vacant(Ty::StationKeys::end_association(association, bank))
    }
    fn association(key: &Self::PairwiseKey) -> &Self::Association {
        &key.association
    }

    type GroupKey = <Ty::StationKeys as KeyTypes<'id>>::GroupKey;
    type GroupPermission = <Ty::StationKeys as KeyTypes<'id>>::GroupPermission;
    type RxMeta = Ty::RxMeta;
}

/// The data machine's tokens on a port of the phase types `Ty`, its MPDUs
/// in buffers of the class `L`.
pub struct PortDataTokens<'id, Ty, L>(Marker<'id, Ty, L>);

impl<'id, Ty, L> StaDataTokens for PortDataTokens<'id, Ty, L>
where
    Ty: PhaseTypes<'id, TxBody = core::convert::Infallible> + TxBufferPool<'id, L>,
    L: TxBufferClass,
{
    type Class = L;
    type EpochPath = Ty::EpochPath;
    type Ticket = Slot<'id, Ty::TicketKind, Ty::TicketPath>;
    type Buffer = <Ty as TxBufferPool<'id, L>>::Buffer;
    type Sent = <Ty as TxBufferPool<'id, L>>::Sent;
    type AckEvidence = Ty::AckEvidence;
    type AirDenied = Ty::AirDenied;

    fn renew(mpdu: Self::Sent) -> Self::Buffer {
        let (memory, body) = <Ty as TxBufferPool<'id, L>>::renew(mpdu);
        if let Some(body) = body {
            match body {}
        }
        memory
    }
}
