use super::*;
use oer_ieee80211_port::association::{self as association, *};
use oer_ieee80211_wire::security::Spp;

#[derive(Clone, Copy, Debug)]
pub struct RawKeyIdentity {
    pub(super) role: VifRole,
    pub(super) slot: u8,
    pub(super) serial: u64,
    pub(super) spp: Spp,
    pub(super) key_id: GroupKeyId,
    pub(super) activity: &'static KeyActivity,
}
impl PartialEq for RawKeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial && core::ptr::eq(self.activity, other.activity)
    }
}
impl Eq for RawKeyIdentity {}
impl RawKeyIdentity {
    pub(super) fn active(&self) -> bool {
        self.activity.matches(self.serial)
    }
}
/// A read-only identity bound to both the slot and this body's loan.
#[derive(Debug)]
pub struct KeyIdentity<'p, Id: SlotIdentity> {
    pub(super) owned: RawKeyIdentity,
    pub(super) brand: PhantomData<fn(&'p (), Id) -> (&'p (), Id)>,
}
impl<Id: SlotIdentity> Clone for KeyIdentity<'_, Id> {
    fn clone(&self) -> Self {
        Self {
            owned: self.owned,
            brand: PhantomData,
        }
    }
}
impl<Id: SlotIdentity> PartialEq for KeyIdentity<'_, Id> {
    fn eq(&self, other: &Self) -> bool {
        self.owned == other.owned
    }
}
impl<Id: SlotIdentity> Eq for KeyIdentity<'_, Id> {}
impl<Id: SlotIdentity> KeyIdentity<'_, Id> {
    pub fn role(&self) -> VifRole {
        self.owned.role
    }
    pub fn key_id(&self) -> GroupKeyId {
        self.owned.key_id
    }
}
pub struct PairwiseKey<'id, Id: SlotIdentity, R> {
    pub(super) identity: RawKeyIdentity,
    pub(super) brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id, R)>,
}
pub struct GroupKey<'id, Id: SlotIdentity, R> {
    pub(super) identity: RawKeyIdentity,
    pub(super) brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id, R)>,
}
pub struct PortKeys<Id: SlotIdentity, R>(PhantomData<fn(Id) -> (Id, R)>);
pub(super) type StaPath<Id: SlotIdentity> = Child<CorePath<Id>, 2>;
pub(super) type ApPath<Id: SlotIdentity> = Child<CorePath<Id>, 3>;
pub(super) type StaPermission<'id, Id: SlotIdentity> =
    Slot<'id, GroupPermission<Station>, Part<Id, 5>>;
pub(super) type ApPermission<'id, Id: SlotIdentity> =
    Slot<'id, GroupPermission<AccessPoint>, Part<Id, 6>>;

macro_rules! keys {
    ($role:ty, $p:literal, $g:literal, $path:ty, $permission:ty) => {
        impl<'id, Id: SlotIdentity + 'id> KeyTypes<'id> for PortKeys<Id, $role> {
            type Pool = AssociationPool<$role, $p, $g>;
            type AssociationStart = AssociationStart<'id, $role, $p, $g, $path>;
            type Association = Association<'id, $role, $p, $g, $path>;
            type VacantKeyBank = VacantKeyBank<'id, $role, $p, $g, $path>;
            type VacantAssociation = VacantAssociation<'id, $role, $p, $g, $path>;
            type PairwiseSlot = PairwiseKeySlot<'id, $role, $p, $g, $path>;
            type GroupSlot = GroupKeySlot<'id, $role, $p, $g, $path>;
            type PairwiseKey = PairwiseKey<'id, Id, $role>;
            type GroupKey = GroupKey<'id, Id, $role>;
            type GroupPermission = $permission;
            type Identity = KeyIdentity<'id, Id>;
            type GroupSlotIdentity = u8;
            fn begin_association(start: Self::AssociationStart, spp: Spp) -> Self::Association {
                start.begin(spp)
            }
            fn collect_key_bank<const N: usize>(
                parts: KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
            ) -> Result<Self::VacantKeyBank, KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>>
            {
                VacantKeyBank::collect(parts)
            }
            fn end_association(
                association: Self::Association,
                vacant: Self::VacantKeyBank,
            ) -> Self::VacantAssociation {
                association::end_association(association, vacant)
            }
            fn split_vacant<const N: usize>(
                vacant: Self::VacantAssociation,
            ) -> (
                Self::AssociationStart,
                Self::PairwiseSlot,
                KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
            ) {
                const {
                    let pairwise = $p - 1;
                    let group = $g;
                    let required = if pairwise > group { pairwise } else { group };
                    assert!(N >= required, "key bank must fit the association pool");
                }
                let (start, bank) = vacant.into_parts();
                let (pairwise, group) = bank.into_parts();
                let mut pairwise = pairwise.into_iter();
                let first = pairwise.next().expect("association has a PTK slot");
                let mut parts = KeyBankParts {
                    pairwise: heapless::Vec::new(),
                    group: heapless::Vec::new(),
                };
                for slot in pairwise {
                    if let Err(_slot) = parts.pairwise.push(slot) {
                        panic!("consumer key bank fits the advertised association pool");
                    }
                }
                for slot in group {
                    if let Err(_slot) = parts.group.push(slot) {
                        panic!("consumer key bank fits the advertised association pool");
                    }
                }
                (start, first, parts)
            }
            fn pairwise_identity(key: &Self::PairwiseKey) -> Self::Identity {
                KeyIdentity {
                    owned: key.identity,
                    brand: PhantomData,
                }
            }
            fn group_identity(key: &Self::GroupKey) -> Self::Identity {
                KeyIdentity {
                    owned: key.identity,
                    brand: PhantomData,
                }
            }
            fn group_slot_identity(key: &Self::GroupKey) -> u8 {
                key.identity.slot
            }
            fn vacant_group_slot_identity(slot: &Self::GroupSlot) -> u8 {
                slot.index()
            }
        }
    };
}
keys!(Station, 1, 1, StaPath<Id>, StaPermission<'id, Id>);
keys!(AccessPoint, 15, 3, ApPath<Id>, ApPermission<'id, Id>);

pub(super) enum Installation<'id, Id: SlotIdentity> {
    Transmit {
        spp: Spp,
        scope: TransmitKeyScope,
        slot: core_mac::association::VacantKeySlot<'id, CorePath<Id>>,
    },
    Receive {
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
        key_id: GroupKeyId,
    },
}
pub(super) enum Installed<'id, Id: SlotIdentity> {
    Transmit(KeyHandle<'id, CorePath<Id>>),
    Receive(ReceiveKeyHandle<'id, CorePath<Id>>),
}
pub(super) enum Removal<'id, Id: SlotIdentity> {
    StationPairwise(core_mac::association::StationAssociation<'id, CorePath<Id>>),
    AccessPointPairwise(core_mac::association::AccessPointAssociation<'id, CorePath<Id>>),
    StationGroup(StaPermission<'id, Id>),
    AccessPointGroup(ApPermission<'id, Id>),
}
pub(super) enum GroupCommand<'id, Id: SlotIdentity> {
    InstallStation {
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
        permission: StaPermission<'id, Id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    InstallAccessPoint {
        spp: Spp,
        slot: core_mac::association::AccessPointGroupSlot<'id, CorePath<Id>>,
        permission: ApPermission<'id, Id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    Replace {
        permission: StaPermission<'id, Id>,
        key: GroupKey<'id, Id, Station>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    Select {
        permission: StaPermission<'id, Id>,
        key: GroupKey<'id, Id, Station>,
    },
    Remove {
        identity: RawKeyIdentity,
        removal: Removal<'id, Id>,
    },
}

pub(super) trait Role<'id, Id: SlotIdentity + 'id>: InterfaceRole + Sized
where
    PortKeys<Id, Self>: KeyTypes<'id, Identity = KeyIdentity<'id, Id>>,
{
    fn identity(
        association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
        slot: &<PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseSlot,
        serial: u64,
        activity: &'static [KeyActivity; 20],
    ) -> RawKeyIdentity;
    fn pairwise(
        association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
    ) -> Installation<'id, Id>;
    fn group(
        association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupSlot,
        permission: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupPermission,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id, Id>;
    fn removal(association: <PortKeys<Id, Self> as KeyTypes<'id>>::Association)
    -> Removal<'id, Id>;
    fn remove_group(
        permission: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupPermission,
    ) -> Removal<'id, Id>;
    fn pairwise_key(identity: RawKeyIdentity)
    -> <PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseKey;
    fn group_identity(key: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupKey) -> RawKeyIdentity;
}

macro_rules! role {
    ($role:ty, $p:literal, $g:literal, $path:ty, $install:ident, $remove:ident, $group_remove:ident) => {
        impl<'id, Id: SlotIdentity + 'id> Role<'id, Id> for $role {
            fn identity(
                association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
                slot: &<PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseSlot,
                serial: u64,
                activity: &'static [KeyActivity; 20],
            ) -> RawKeyIdentity {
                RawKeyIdentity {
                    role: Self::ROLE,
                    slot: slot.index(),
                    serial,
                    spp: association.spp(),
                    key_id: GroupKeyId::new(0).expect("wire KeyID zero"),
                    activity: &activity[if Self::ROLE == VifRole::Station {
                        0
                    } else {
                        2 + usize::from(slot.index())
                    }],
                }
            }
            fn pairwise(
                association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
                slot: <PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseSlot,
                peer: IndividualAddress,
            ) -> Installation<'id, Id> {
                let (spp, scope, slot) = core_mac::association::TransmitKeyInstallation::$install {
                    association,
                    slot,
                    peer: peer.get(),
                }
                .into_parts();
                Installation::Transmit { spp, scope, slot }
            }
            fn removal(
                association: <PortKeys<Id, Self> as KeyTypes<'id>>::Association,
            ) -> Removal<'id, Id> {
                Removal::$remove(association)
            }
            fn remove_group(
                permission: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupPermission,
            ) -> Removal<'id, Id> {
                Removal::$group_remove(permission)
            }
            fn pairwise_key(identity: RawKeyIdentity) -> PairwiseKey<'id, Id, Self> {
                PairwiseKey {
                    identity,
                    brand: PhantomData,
                }
            }
            fn group_identity(key: GroupKey<'id, Id, Self>) -> RawKeyIdentity {
                key.identity
            }
            fn group(
                association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
                slot: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupSlot,
                permission: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupPermission,
                key_id: GroupKeyId,
                material: Ccmp128Key,
            ) -> GroupCommand<'id, Id> {
                Self::group_command(association.spp(), slot, permission, key_id, material)
            }
        }
    };
}
role!(
    Station,
    1,
    1,
    StaPath<Id>,
    StationPairwise,
    StationPairwise,
    StationGroup
);
role!(
    AccessPoint,
    15,
    3,
    ApPath<Id>,
    AccessPointPairwise,
    AccessPointPairwise,
    AccessPointGroup
);

trait GroupRole<'id, Id: SlotIdentity + 'id>: InterfaceRole + Sized
where
    PortKeys<Id, Self>: KeyTypes<'id>,
{
    fn group_command(
        spp: Spp,
        slot: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupSlot,
        permission: <PortKeys<Id, Self> as KeyTypes<'id>>::GroupPermission,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id, Id>;
}
impl<'id, Id: SlotIdentity + 'id> GroupRole<'id, Id> for Station {
    fn group_command(
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
        permission: StaPermission<'id, Id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id, Id> {
        GroupCommand::InstallStation {
            spp,
            slot,
            permission,
            key_id,
            material,
        }
    }
}
impl<'id, Id: SlotIdentity + 'id> GroupRole<'id, Id> for AccessPoint {
    fn group_command(
        spp: Spp,
        slot: core_mac::association::AccessPointGroupSlot<'id, CorePath<Id>>,
        permission: ApPermission<'id, Id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id, Id> {
        GroupCommand::InstallAccessPoint {
            spp,
            slot,
            permission,
            key_id,
            material,
        }
    }
}
