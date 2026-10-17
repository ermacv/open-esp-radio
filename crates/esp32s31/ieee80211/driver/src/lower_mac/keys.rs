//! Installed-key handles and the affine rights of admitted TX attempts.

use core::marker::PhantomData;
use oer_radio_port::{PoolPath, Root};

use super::{
    ApGroupCcmpSlot, ApPairwiseCcmpSlot, CheckedKeyScope, KeyToken, MacAddress, StaGroupCcmpSlot,
    StaPairwiseCcmpSlot, TxTicket,
};

type Installation<'core, Path> = PhantomData<fn(&'core (), Path) -> (&'core (), Path)>;

/// The sole handle of an installed key. Removing it consumes this value;
/// further admission then fails to compile. Already admitted attempts own
/// independent `KeyUse` values and may finish on different EDCA queues.
#[must_use = "remove the installed key through its core"]
pub struct KeyHandle<'core, Path: PoolPath = Root> {
    pub(super) slot: super::association::VacantKeySlot<'core, Path>,
    pub(super) index: usize,
    pub(super) token: TransmitKeyToken,
    pub(super) scope: TransmitKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

/// Keys which can encrypt transmitted frames. Station GTKs have a different
/// installation operation and handle, so they cannot enter TX admission or PN
/// allocation even while they remain installed for reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitKeyScope {
    StationPairwise {
        peer: MacAddress,
    },
    AccessPointPairwise {
        peer: MacAddress,
        association_id: u16,
    },
    AccessPointGroup {
        key_id: oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId,
    },
}

impl From<TransmitKeyScope> for CheckedKeyScope {
    fn from(scope: TransmitKeyScope) -> Self {
        match scope {
            TransmitKeyScope::StationPairwise { peer } => Self::StationPairwise { peer },
            TransmitKeyScope::AccessPointPairwise {
                peer,
                association_id,
            } => Self::AccessPointPairwise {
                peer,
                association_id,
            },
            TransmitKeyScope::AccessPointGroup { key_id } => Self::AccessPointGroup { key_id },
        }
    }
}

pub(super) enum TransmitKeyToken {
    StaPairwise(StaPairwiseCcmpSlot),
    ApPairwise(ApPairwiseCcmpSlot),
    ApGroup(ApGroupCcmpSlot),
}

impl TransmitKeyToken {
    pub(super) const fn hardware_index(&self) -> u8 {
        match self {
            Self::StaPairwise(slot) => slot.hardware_index(),
            Self::ApPairwise(slot) => slot.hardware_index(),
            Self::ApGroup(slot) => slot.hardware_index(),
        }
    }
}

/// An installed station group key, usable only for receive and removal.
/// Neither `admit_key` nor `number` accepts this owner.
#[must_use = "remove the receive key through its core"]
pub struct ReceiveKeyHandle<'core, Path: PoolPath = Root> {
    pub(super) slot: super::association::VacantKeySlot<'core, Path>,
    pub(super) index: usize,
    pub(super) token: StaGroupCcmpSlot,
    pub(super) key_id: oer_ieee80211_mac::ccmp::GroupKeyId,
    pub(super) brand: Installation<'core, Path>,
}

/// A consumed installed handle, retained until hardware removal completes.
/// Converting either handle into this owner permanently ends new TX admission.
#[must_use = "submit the consumed key to removal"]
pub struct RemovingKey<'core, Path: PoolPath = Root> {
    pub(super) slot: super::association::VacantKeySlot<'core, Path>,
    pub(super) index: usize,
    pub(super) token: KeyToken,
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<'core, Path: PoolPath> From<KeyHandle<'core, Path>> for RemovingKey<'core, Path> {
    fn from(key: KeyHandle<'core, Path>) -> Self {
        Self {
            slot: key.slot,
            index: key.index,
            token: match key.token {
                TransmitKeyToken::StaPairwise(slot) => KeyToken::StaPairwise(slot),
                TransmitKeyToken::ApPairwise(slot) => KeyToken::ApPairwise(slot),
                TransmitKeyToken::ApGroup(slot) => KeyToken::ApGroup(slot),
            },
            scope: key.scope.into(),
            brand: key.brand,
        }
    }
}

impl<'core, Path: PoolPath> From<ReceiveKeyHandle<'core, Path>> for RemovingKey<'core, Path> {
    fn from(key: ReceiveKeyHandle<'core, Path>) -> Self {
        Self {
            slot: key.slot,
            index: key.index,
            token: KeyToken::StaGroup(key.token),
            scope: CheckedKeyScope::StationGroup { key_id: key.key_id },
            brand: key.brand,
        }
    }
}

/// Count bounded by the same installation layout that issues the tickets.
/// Issuing a right consumes a ticket; decrementing consumes that right. These
/// are the only mutations, so even concurrent queues cannot exceed TICKETS.
pub(super) struct KeyUseCount<const TICKETS: usize>(u16);

impl<const TICKETS: usize> KeyUseCount<TICKETS> {
    pub(super) const WIDTH_CHECK: () = assert!(TICKETS <= u16::MAX as usize);

    pub(super) const fn new() -> Self {
        let () = Self::WIDTH_CHECK;
        Self(0)
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub(super) fn admit<'core, Path: PoolPath>(
        &mut self,
        key: &KeyHandle<'core, Path>,
        ticket: TxTicket<'core, Path>,
    ) -> KeyUse<'core, Path> {
        self.0 += 1;
        KeyUse {
            ticket,
            index: key.index,
            selector: key.token.hardware_index(),
            role: CheckedKeyScope::from(key.scope).role(),
        }
    }

    pub(super) fn release<'core, Path: PoolPath>(
        &mut self,
        key: KeyUse<'core, Path>,
    ) -> TxTicket<'core, Path> {
        self.0 -= 1;
        key.ticket
    }

    #[cfg(test)]
    pub(super) const fn get(&self) -> u16 {
        self.0
    }
}

/// Permission to use the installed CCMP material for one admitted attempt.
///
/// Only admission through the installing core issues this right. It owns a
/// TX ticket, carries the installation brand and identifies both the logical
/// key slot and its programmed hardware selector. Publication reads that
/// selector and MIC size directly from the right; it never resolves a copied
/// key index. The secret material remains in that programmed hardware slot
/// until every right returns. No borrowed pointer into the core is retained.
///
/// The right travels with a held, published or quarantined attempt. A normal
/// terminal returns it exactly once. Poisoned keeps it in quarantine: neither
/// clearing the slot nor reporting `KeyRemoved` is permitted there.
#[must_use = "return the right only with its attempt's terminal or refusal"]
pub struct KeyUse<'core, Path: PoolPath = Root> {
    pub(super) ticket: TxTicket<'core, Path>,
    pub(super) index: usize,
    pub(super) selector: u8,
    pub(super) role: oer_ieee80211_lower_mac::VifRole,
}

/// The encryption authority of an admitted attempt. The protected variant
/// cannot be constructed from an index, a handle or a bare ticket.
#[must_use = "carry the admission to its terminal"]
pub enum TxAdmission<'core, Path: PoolPath = Root> {
    /// An unencrypted attempt still consumes one admission ticket.
    Open(TxTicket<'core, Path>),
    /// The right to use an installed key for this attempt alone.
    Protected(KeyUse<'core, Path>),
}

impl<Path: PoolPath> TxAdmission<'_, Path> {
    pub(super) fn matches_frame(
        &self,
        role: oer_ieee80211_lower_mac::VifRole,
        frame: Option<&[u8]>,
    ) -> bool {
        let Some(field) = frame.and_then(|frame| frame.first_chunk::<2>()) else {
            return false;
        };
        let control = oer_ieee80211_mac::frame::FrameControl::new(u16::from_le_bytes(*field));
        match self {
            Self::Open(_) => control.is_none_or(|control| !control.protected()),
            Self::Protected(key) => {
                key.role == role && control.is_some_and(|control| control.protected())
            }
        }
    }

    pub(super) const fn hardware(&self) -> (u8, usize) {
        match self {
            Self::Open(_) => (0, 0),
            Self::Protected(key) => (key.selector, crate::ordinary_tx::TX_CCMP_MIC_SIZE),
        }
    }
}

/// The key was cleared after its last admitted use returned. This receipt is
/// emitted at removal's FIFO position if no uses exist, otherwise immediately
/// after the last TX terminal. There is no polling step to discover completion.
#[must_use = "complete the corresponding portable key-removal request"]
pub struct KeyRemoved<'core, Path: PoolPath = Root> {
    pub(super) slot: super::association::VacantKeySlot<'core, Path>,
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<'core, Path: PoolPath> KeyRemoved<'core, Path> {
    /// Recover the slot only after hardware removal and every admitted TX use.
    pub fn into_slot(self) -> super::association::VacantKeySlot<'core, Path> {
        self.slot
    }

    /// The installation's key which has now left the hardware.
    pub const fn scope(&self) -> CheckedKeyScope {
        self.scope
    }
}

/// Admission did not grant a key right; the caller retains its ticket.
#[must_use = "retain the returned ticket and handle the refusal"]
pub struct KeyAdmissionRefused<'core, Path: PoolPath = Root> {
    pub ticket: TxTicket<'core, Path>,
    pub reason: KeyAdmissionError,
}

/// Why an installed key cannot admit a TX attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAdmissionError {
    /// The poisoned core retains its key material until reset.
    Poisoned,
}

/// An admission returned after the core was poisoned remains in custody.
/// It cannot be submitted again or turned into a reusable ticket.
#[must_use = "retain the admission in the poisoned backend's resource bank"]
pub struct QuarantinedAdmission<'core, Path: PoolPath = Root> {
    pub(super) _admission: TxAdmission<'core, Path>,
}
