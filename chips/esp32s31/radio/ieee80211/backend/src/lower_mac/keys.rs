//! Installed-key handles and the affine rights of admitted TX attempts.

use core::marker::PhantomData;
use oer_protocols_radio_port::{PoolPath, Root};

use super::{
    ApGroupCcmpSlot, ApPairwiseCcmpSlot, CheckedKeyScope, KeyToken, StaGroupCcmpSlot,
    StaPairwiseCcmpSlot, TxTicket,
};

type Installation<'core, Path> = PhantomData<fn(&'core (), Path) -> (&'core (), Path)>;

/// The sole handle of an installed key. Removing it consumes this value;
/// further admission then fails to compile. Already admitted attempts own
/// independent `KeyUse` values and may finish on different EDCA queues.
#[must_use = "remove the installed key through its core"]
pub struct KeyHandle<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    pub(super) slot: super::association::VacantKeySlot<'core, Path>,
    pub(super) index: usize,
    pub(super) token: TransmitKeyToken,
    pub(super) scope: TransmitKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<Path: PoolPath> KeyHandle<'_, Path> {
    /// The role and kind of the slot the key holds, which its removal's
    /// receipt returns.
    pub const fn slot_kind(&self) -> super::association::KeySlotKind {
        self.slot.kind()
    }
}

/// Keys which can encrypt transmitted frames. Station GTKs have a different
/// installation operation and handle, so they cannot enter TX admission or PN
/// allocation even while they remain installed for reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitKeyScope {
    StationPairwise {
        peer: [u8; 6],
    },
    AccessPointPairwise {
        peer: [u8; 6],
        association_id: u16,
    },
    AccessPointGroup {
        key_id: crate::mac::crypto::ApGroupKeyId,
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
pub struct ReceiveKeyHandle<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>>
{
    pub(super) slot: super::association::StationGroupSlot<'core, Path>,
    pub(super) index: usize,
    pub(super) token: StaGroupCcmpSlot,
    pub(super) material: crate::mac::crypto::StaGroupCcmpKeyMaterial,
    pub(super) key_id: oer_protocols_ieee80211_wire::ccmp::GroupKeyId,
    pub(super) brand: Installation<'core, Path>,
}

impl<Path: PoolPath> ReceiveKeyHandle<'_, Path> {
    /// The role and kind of the slot the key holds.
    pub const fn slot_kind(&self) -> super::association::KeySlotKind {
        super::association::KeySlotKind::StationGroup
    }
}

/// A consumed installed handle, retained until hardware removal completes.
/// Converting either handle into this owner permanently ends new TX admission.
/// Its slot is of one kind `S`, which its receipt returns.
#[must_use = "submit the consumed key to removal"]
pub struct RemovingKey<'core, Path: PoolPath, S> {
    pub(super) slot: S,
    pub(super) index: usize,
    pub(super) token: KeyToken,
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<'core, Path: PoolPath> KeyHandle<'core, Path> {
    /// The handle consumed for removal, when its slot is of kind `S`;
    /// otherwise the handle comes back unchanged.
    ///
    /// # Errors
    ///
    /// The slot is of another kind: the handle, whole.
    pub fn into_removing<S: RemovableSlot<'core, Path>>(
        self,
    ) -> Result<RemovingKey<'core, Path, S>, Self> {
        match S::from_vacant(self.slot) {
            Ok(slot) => Ok(RemovingKey {
                slot,
                index: self.index,
                token: match self.token {
                    TransmitKeyToken::StaPairwise(slot) => KeyToken::StaPairwise(slot),
                    TransmitKeyToken::ApPairwise(slot) => KeyToken::ApPairwise(slot),
                    TransmitKeyToken::ApGroup(slot) => KeyToken::ApGroup(slot),
                },
                scope: self.scope.into(),
                brand: self.brand,
            }),
            Err(slot) => Err(Self { slot, ..self }),
        }
    }
}

impl<'core, Path: PoolPath> ReceiveKeyHandle<'core, Path> {
    /// The handle consumed for removal, when `S` is the station's group
    /// slot; otherwise the handle comes back unchanged.
    ///
    /// # Errors
    ///
    /// `S` is another kind: the handle, whole.
    pub fn into_removing<S: RemovableSlot<'core, Path>>(
        self,
    ) -> Result<RemovingKey<'core, Path, S>, Self> {
        match S::from_station_group(self.slot) {
            Ok(slot) => Ok(RemovingKey {
                slot,
                index: self.index,
                token: KeyToken::StaGroup(self.token),
                scope: CheckedKeyScope::StationGroup {
                    key_id: self.key_id,
                },
                brand: self.brand,
            }),
            Err(slot) => Err(Self { slot, ..self }),
        }
    }
}

/// A key slot type whose removal the core keeps in places of its own kind:
/// a removal's place and its receipt name the slot type, so a receipt of one
/// kind cannot answer a removal of another.
pub trait RemovableSlot<'core, Path: PoolPath>: Sized + removals::Places<'core, Path> {
    /// The role and kind of every slot of this type.
    const KIND: super::association::KeySlotKind;
    /// The slot in the form of every kind.
    fn into_vacant(self) -> super::association::VacantKeySlot<'core, Path>;
    /// A slot of this kind, or the slot back.
    ///
    /// # Errors
    ///
    /// The slot is of another kind.
    fn from_vacant(
        slot: super::association::VacantKeySlot<'core, Path>,
    ) -> Result<Self, super::association::VacantKeySlot<'core, Path>>;
    /// The station's group slot as this kind, or the slot back.
    ///
    /// # Errors
    ///
    /// This kind is not the station's group slot.
    fn from_station_group(
        slot: super::association::StationGroupSlot<'core, Path>,
    ) -> Result<Self, super::association::StationGroupSlot<'core, Path>>;
}

macro_rules! removable {
    ($slot:ident, $kind:ident, $field:ident, $group:expr) => {
        impl<'core, Path: PoolPath> RemovableSlot<'core, Path>
            for super::association::$slot<'core, Path>
        {
            const KIND: super::association::KeySlotKind = super::association::KeySlotKind::$kind;
            fn into_vacant(self) -> super::association::VacantKeySlot<'core, Path> {
                super::association::VacantKeySlot::$kind(self)
            }
            fn from_vacant(
                slot: super::association::VacantKeySlot<'core, Path>,
            ) -> Result<Self, super::association::VacantKeySlot<'core, Path>> {
                match slot {
                    super::association::VacantKeySlot::$kind(slot) => Ok(slot),
                    other => Err(other),
                }
            }
            fn from_station_group(
                slot: super::association::StationGroupSlot<'core, Path>,
            ) -> Result<Self, super::association::StationGroupSlot<'core, Path>> {
                $group(slot)
            }
        }
        impl<'core, Path: PoolPath> removals::Places<'core, Path>
            for super::association::$slot<'core, Path>
        {
            fn places<'a>(
                removals: &'a mut removals::Removals<'core, Path>,
            ) -> &'a mut [Option<removals::Removal<'core, Path, Self>>] {
                &mut removals.$field
            }
            fn places_ref<'a>(
                removals: &'a removals::Removals<'core, Path>,
            ) -> &'a [Option<removals::Removal<'core, Path, Self>>] {
                &removals.$field
            }
        }
    };
}
removable!(StationPairwiseSlot, StationPairwise, station_pairwise, Err);
removable!(StationGroupSlot, StationGroup, station_group, Ok);
removable!(
    AccessPointPairwiseSlot,
    AccessPointPairwise,
    access_point_pairwise,
    Err
);
removable!(
    AccessPointGroupSlot,
    AccessPointGroup,
    access_point_group,
    Err
);

/// A removal the core keeps in a place of its slot's kind: what
/// [`super::LowerMacCore::take_removed`] takes its receipt from.
#[must_use = "take the removal's receipt from its core"]
pub struct RemovalPlace<'core, Path: PoolPath, S> {
    pub(super) position: usize,
    pub(super) brand: PlaceBrand<'core, Path, S>,
}
/// A removal place's brand: its core's installation and its slot kind.
type PlaceBrand<'core, Path, S> = PhantomData<fn(&'core (), Path, S) -> (&'core (), Path, S)>;

/// Where the core keeps its removals: one array per slot kind, as long as
/// that kind's pool, so each removal stays with a slot of its own kind.
pub mod removals {
    use super::{KeyRemoved, RemovingKey};
    use crate::lower_mac::association::{
        ACCESS_POINT_GROUP_SLOTS, ACCESS_POINT_PAIRWISE_SLOTS, AccessPointGroupSlot,
        AccessPointPairwiseSlot, STATION_GROUP_SLOTS, STATION_PAIRWISE_SLOTS, StationGroupSlot,
        StationPairwiseSlot,
    };
    use oer_protocols_radio_port::PoolPath;

    /// A key's removal in its place: waiting for its last use, or finished
    /// with a receipt its sink has not taken yet.
    pub enum Removal<'core, Path: PoolPath, S> {
        /// The consumed handle, waiting for the key's last admitted use.
        Pending(RemovingKey<'core, Path, S>),
        /// The key is cleared; the receipt waits for its sink.
        Finished(KeyRemoved<'core, Path, S>),
    }

    /// The core's removals, by slot kind.
    pub struct Removals<'core, Path: PoolPath> {
        pub(in crate::lower_mac) station_pairwise: [Option<
            Removal<'core, Path, StationPairwiseSlot<'core, Path>>,
        >; STATION_PAIRWISE_SLOTS],
        pub(in crate::lower_mac) station_group:
            [Option<Removal<'core, Path, StationGroupSlot<'core, Path>>>; STATION_GROUP_SLOTS],
        pub(in crate::lower_mac) access_point_pairwise: [Option<
            Removal<'core, Path, AccessPointPairwiseSlot<'core, Path>>,
        >; ACCESS_POINT_PAIRWISE_SLOTS],
        pub(in crate::lower_mac) access_point_group: [Option<
            Removal<'core, Path, AccessPointGroupSlot<'core, Path>>,
        >; ACCESS_POINT_GROUP_SLOTS],
    }

    impl<Path: PoolPath> Removals<'_, Path> {
        pub(in crate::lower_mac) const fn new() -> Self {
            Self {
                station_pairwise: [const { None }; STATION_PAIRWISE_SLOTS],
                station_group: [const { None }; STATION_GROUP_SLOTS],
                access_point_pairwise: [const { None }; ACCESS_POINT_PAIRWISE_SLOTS],
                access_point_group: [const { None }; ACCESS_POINT_GROUP_SLOTS],
            }
        }

        /// Whether any removal is kept.
        pub(in crate::lower_mac) fn any(&self) -> bool {
            self.station_pairwise.iter().any(Option::is_some)
                || self.station_group.iter().any(Option::is_some)
                || self.access_point_pairwise.iter().any(Option::is_some)
                || self.access_point_group.iter().any(Option::is_some)
        }
    }

    /// The places of one slot kind.
    pub trait Places<'core, Path: PoolPath>: Sized {
        /// This kind's places.
        fn places<'a>(
            removals: &'a mut Removals<'core, Path>,
        ) -> &'a mut [Option<Removal<'core, Path, Self>>];
        /// This kind's places, to read.
        fn places_ref<'a>(
            removals: &'a Removals<'core, Path>,
        ) -> &'a [Option<Removal<'core, Path, Self>>];
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
pub struct KeyUse<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    pub(super) ticket: TxTicket<'core, Path>,
    pub(super) index: usize,
    pub(super) selector: u8,
    pub(super) role: oer_protocols_ieee80211_contracts_port::VifRole,
}

/// The encryption authority of an admitted attempt. The protected variant
/// cannot be constructed from an index, a handle or a bare ticket.
#[must_use = "carry the admission to its terminal"]
pub enum TxAdmission<'core, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>> {
    /// An unencrypted attempt still consumes one admission ticket.
    Open(TxTicket<'core, Path>),
    /// The right to use an installed key for this attempt alone.
    Protected(KeyUse<'core, Path>),
}

impl<'core, Path: PoolPath> TxAdmission<'core, Path> {
    /// The ticket of an admission whose core holds no key counts.
    pub(in crate::lower_mac) fn into_ticket(self) -> TxTicket<'core, Path> {
        match self {
            Self::Open(ticket) => ticket,
            Self::Protected(key) => key.ticket,
        }
    }
}

impl<Path: PoolPath> TxAdmission<'_, Path> {
    pub(super) fn matches_frame(
        &self,
        role: oer_protocols_ieee80211_contracts_port::VifRole,
        frame: Option<&[u8]>,
    ) -> bool {
        let Some(field) = frame.and_then(|frame| frame.first_chunk::<2>()) else {
            return false;
        };
        let control =
            oer_protocols_ieee80211_wire::frame::FrameControl::new(u16::from_le_bytes(*field));
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
/// Its slot is of one kind `S`; [`KeyRemoved::erase`] gives the form of every
/// kind.
#[must_use = "complete the corresponding portable key-removal request"]
pub struct KeyRemoved<
    'core,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>,
    S = super::association::VacantKeySlot<'core, Path>,
> {
    pub(super) slot: S,
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<'core, Path: PoolPath, S> KeyRemoved<'core, Path, S> {
    /// Recover the slot only after hardware removal and every admitted TX use.
    pub fn into_slot(self) -> S {
        self.slot
    }

    /// The installation's key which has now left the hardware.
    pub const fn scope(&self) -> CheckedKeyScope {
        self.scope
    }
}

impl<'core, Path: PoolPath, S: RemovableSlot<'core, Path>> KeyRemoved<'core, Path, S> {
    /// The receipt with its slot in the form of every kind.
    pub fn erase(self) -> KeyRemoved<'core, Path> {
        KeyRemoved {
            slot: self.slot.into_vacant(),
            scope: self.scope,
            brand: self.brand,
        }
    }
}

/// Admission did not grant a key right; the caller retains its ticket.
#[must_use = "retain the returned ticket and handle the refusal"]
pub struct KeyAdmissionRefused<
    'core,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'core>>,
> {
    pub ticket: TxTicket<'core, Path>,
    pub reason: KeyAdmissionError,
}

/// Why an installed key cannot admit a TX attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAdmissionError {
    /// The poisoned core retains its key material until reset.
    Poisoned,
}

/// An installed handle of either type, for tests that remove any key.
#[cfg(test)]
pub(crate) enum InstalledForTest<'core, Path: PoolPath> {
    Transmit(KeyHandle<'core, Path>),
    Receive(ReceiveKeyHandle<'core, Path>),
}
#[cfg(test)]
impl<'core, Path: PoolPath> From<KeyHandle<'core, Path>> for InstalledForTest<'core, Path> {
    fn from(key: KeyHandle<'core, Path>) -> Self {
        Self::Transmit(key)
    }
}
#[cfg(test)]
impl<'core, Path: PoolPath> From<ReceiveKeyHandle<'core, Path>> for InstalledForTest<'core, Path> {
    fn from(key: ReceiveKeyHandle<'core, Path>) -> Self {
        Self::Receive(key)
    }
}
