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
use oer_ieee80211_port::{
    AccessPoint, Station,
    association::{Association, GroupKeySlot, PairwiseKeySlot, VacantAssociation},
};
use oer_ieee80211_wire::security::Spp;
use oer_radio_port::{Child, PoolPath, Root};

/// The STA interface owns one PTK and one receive-only GTK slot.
pub type StationAssociation<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    Association<'id, Station, 1, 1, Child<P, 2>>;
/// All AP peers share the interface's one SPP policy.
pub type AccessPointAssociation<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    Association<'id, AccessPoint, 15, 3, Child<P, 3>>;
pub type VacantStationAssociation<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    VacantAssociation<'id, Station, 1, 1, Child<P, 2>>;
pub type VacantAccessPointAssociation<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    VacantAssociation<'id, AccessPoint, 15, 3, Child<P, 3>>;
pub type StationPairwiseSlot<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    PairwiseKeySlot<'id, Station, 1, 1, Child<P, 2>>;
pub type StationGroupSlot<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    GroupKeySlot<'id, Station, 1, 1, Child<P, 2>>;
pub type AccessPointPairwiseSlot<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    PairwiseKeySlot<'id, AccessPoint, 15, 3, Child<P, 3>>;
pub type AccessPointGroupSlot<'id, P = Root<oer_radio_port::LocalId<'id>>> =
    GroupKeySlot<'id, AccessPoint, 15, 3, Child<P, 3>>;

/// An installation cannot name material without the association and its slot.
pub enum TransmitKeyInstallation<'a, 'id, P: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
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
        key_id: oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId,
    },
}

/// A slot retained by an installed key and returned only by KeyRemoved.
#[must_use]
pub enum VacantKeySlot<'id, P: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    StationPairwise(StationPairwiseSlot<'id, P>),
    StationGroup(StationGroupSlot<'id, P>),
    AccessPointPairwise(AccessPointPairwiseSlot<'id, P>),
    AccessPointGroup(AccessPointGroupSlot<'id, P>),
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

/// Installation had no effect; the exact vacant slot remains recoverable.
#[must_use]
pub struct KeyInstallRefused<'id, P: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    pub fault: super::LowerMacFault,
    pub slot: VacantKeySlot<'id, P>,
}

impl<P: PoolPath> core::fmt::Debug for KeyInstallRefused<'_, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeyInstallRefused")
            .field("fault", &self.fault)
            .finish_non_exhaustive()
    }
}
