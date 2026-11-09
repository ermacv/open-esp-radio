//! S31 has one SPP bit per interface; every key borrows that interface's owner.
//!
//! Ending is synchronous after the complete key bank returns. At wifi-lib pin
//! f856b5db55cb18e2d1826044430f3f4089e47cf9, `wDev_remove_KeyEntry` clears its
//! software slot maps and tail-calls `hal_crypto_clr_key_entry`. The latter
//! clears validity and all ten key words, without changing INTERFACE_CONTROL.
//! Thus KeyRemoved has already completed every hardware operation. With no
//! valid keys the retained SPP bit is irrelevant; the next association's
//! decision is programmed before its first PTK. No end command is necessary.
//! SOURCE(esp32s31): `libpp[if_hwctrl.o]::ic_set_key` supplies the association
//! decision to `libpp[wdev.o]::wDev_Insert_KeyEntry`, which calls
//! `libpp[hal_crypto.o]::hal_crypto_enable`. Complete removal path:
//! `libpp[wdev.o]::wDev_remove_KeyEntry` +0x0..+0x2e and
//! `libpp[hal_crypto.o]::hal_crypto_clr_key_entry` +0x0..+0x88.

use super::TransmitKeyScope;
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, Station,
    association::{
        Association, AssociationGrant, GroupKeySlot, PairwiseKeySlot, VacantAssociation,
    },
};
use oer_protocols_ieee80211_wire::security::Spp;
use oer_protocols_radio_port::{Child, PoolPath, Root};

/// The station's pairwise key slots.
pub const STATION_PAIRWISE_SLOTS: usize = 1;
/// The station's receive-only group key slots.
pub const STATION_GROUP_SLOTS: usize = 1;
/// The access point's pairwise key slots, one per association.
pub const ACCESS_POINT_PAIRWISE_SLOTS: usize = 15;
/// The access point's group key slots.
pub const ACCESS_POINT_GROUP_SLOTS: usize = 3;

/// The STA interface owns one PTK and one receive-only GTK slot.
pub type StationAssociation<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    Association<'id, Station, STATION_PAIRWISE_SLOTS, STATION_GROUP_SLOTS, Child<P, 2>>;
/// All AP peers share the interface's one SPP policy.
pub type AccessPointAssociation<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    Association<
        'id,
        AccessPoint,
        ACCESS_POINT_PAIRWISE_SLOTS,
        ACCESS_POINT_GROUP_SLOTS,
        Child<P, 3>,
    >;
/// The station's association domain as the mint made it, before the
/// installation issues it.
pub type StationAssociationGrant<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    AssociationGrant<'id, Station, STATION_PAIRWISE_SLOTS, STATION_GROUP_SLOTS, Child<P, 2>>;
/// The access point's association domain as the mint made it.
pub type AccessPointAssociationGrant<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    AssociationGrant<
        'id,
        AccessPoint,
        ACCESS_POINT_PAIRWISE_SLOTS,
        ACCESS_POINT_GROUP_SLOTS,
        Child<P, 3>,
    >;
pub type VacantStationAssociation<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    VacantAssociation<'id, Station, STATION_PAIRWISE_SLOTS, STATION_GROUP_SLOTS, Child<P, 2>>;
pub type VacantAccessPointAssociation<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    VacantAssociation<
        'id,
        AccessPoint,
        ACCESS_POINT_PAIRWISE_SLOTS,
        ACCESS_POINT_GROUP_SLOTS,
        Child<P, 3>,
    >;
pub type StationPairwiseSlot<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    PairwiseKeySlot<'id, Station, STATION_PAIRWISE_SLOTS, STATION_GROUP_SLOTS, Child<P, 2>>;
pub type StationGroupSlot<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    GroupKeySlot<'id, Station, STATION_PAIRWISE_SLOTS, STATION_GROUP_SLOTS, Child<P, 2>>;
pub type AccessPointPairwiseSlot<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> =
    PairwiseKeySlot<
        'id,
        AccessPoint,
        ACCESS_POINT_PAIRWISE_SLOTS,
        ACCESS_POINT_GROUP_SLOTS,
        Child<P, 3>,
    >;
pub type AccessPointGroupSlot<'id, P = Root<oer_protocols_radio_port::LocalId<'id>>> = GroupKeySlot<
    'id,
    AccessPoint,
    ACCESS_POINT_PAIRWISE_SLOTS,
    ACCESS_POINT_GROUP_SLOTS,
    Child<P, 3>,
>;

/// An installation cannot name material without the association and its slot.
pub enum TransmitKeyInstallation<
    'a,
    'id,
    P: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
> {
    StationPairwise {
        association: &'a StationAssociation<'id, P>,
        slot: StationPairwiseSlot<'id, P>,
        peer: [u8; 6],
    },
    AccessPointPairwise {
        association: &'a AccessPointAssociation<'id, P>,
        slot: AccessPointPairwiseSlot<'id, P>,
        peer: [u8; 6],
    },
    AccessPointGroup {
        association: &'a AccessPointAssociation<'id, P>,
        slot: AccessPointGroupSlot<'id, P>,
        key_id: crate::mac::crypto::ApGroupKeyId,
    },
}

/// A slot retained by an installed key and returned only by KeyRemoved.
#[must_use]
pub enum VacantKeySlot<'id, P: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    StationPairwise(StationPairwiseSlot<'id, P>),
    StationGroup(StationGroupSlot<'id, P>),
    AccessPointPairwise(AccessPointPairwiseSlot<'id, P>),
    AccessPointGroup(AccessPointGroupSlot<'id, P>),
}

/// The role and kind of a key slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeySlotKind {
    StationPairwise,
    StationGroup,
    AccessPointPairwise,
    AccessPointGroup,
}

impl<P: PoolPath> VacantKeySlot<'_, P> {
    /// The slot's role and kind.
    pub const fn kind(&self) -> KeySlotKind {
        match self {
            Self::StationPairwise(_) => KeySlotKind::StationPairwise,
            Self::StationGroup(_) => KeySlotKind::StationGroup,
            Self::AccessPointPairwise(_) => KeySlotKind::AccessPointPairwise,
            Self::AccessPointGroup(_) => KeySlotKind::AccessPointGroup,
        }
    }
}

impl<'id, P: PoolPath> From<AccessPointGroupSlot<'id, P>> for VacantKeySlot<'id, P> {
    fn from(slot: AccessPointGroupSlot<'id, P>) -> Self {
        Self::AccessPointGroup(slot)
    }
}

impl<'id, P: PoolPath> From<StationGroupSlot<'id, P>> for VacantKeySlot<'id, P> {
    fn from(slot: StationGroupSlot<'id, P>) -> Self {
        Self::StationGroup(slot)
    }
}

impl<'id, P: PoolPath> TransmitKeyInstallation<'_, 'id, P> {
    pub(crate) fn into_parts(self) -> (Spp, TransmitKeyScope, VacantKeySlot<'id, P>) {
        match self {
            Self::StationPairwise {
                association,
                slot,
                peer,
            } => (
                association.spp(),
                TransmitKeyScope::StationPairwise { peer },
                VacantKeySlot::StationPairwise(slot),
            ),
            Self::AccessPointPairwise {
                association,
                slot,
                peer,
            } => (
                association.spp(),
                TransmitKeyScope::AccessPointPairwise {
                    peer,
                    association_id: u16::from(slot.index()) + 1,
                },
                VacantKeySlot::AccessPointPairwise(slot),
            ),
            Self::AccessPointGroup {
                association,
                slot,
                key_id,
            } => (
                association.spp(),
                TransmitKeyScope::AccessPointGroup { key_id },
                VacantKeySlot::AccessPointGroup(slot),
            ),
        }
    }
}

/// Installation had no effect; the exact vacant slot remains recoverable,
/// as the type the caller gave it (`S`).
#[must_use]
pub struct KeyInstallRefused<
    'id,
    P: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
    S = VacantKeySlot<'id, P>,
> {
    pub fault: super::LowerMacFault,
    pub slot: S,
    pub brand: core::marker::PhantomData<VacantKeySlot<'id, P>>,
}

impl<P: PoolPath, S> core::fmt::Debug for KeyInstallRefused<'_, P, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeyInstallRefused")
            .field("fault", &self.fault)
            .finish_non_exhaustive()
    }
}
