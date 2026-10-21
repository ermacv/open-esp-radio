use super::*;
use oer_ieee80211_port::association::{self as association, *};
use oer_ieee80211_wire::security::Spp;

#[derive(Clone, Copy, Debug)]
pub struct KeyIdentity {
    pub(super) role: VifRole,
    pub(super) slot: u8,
    pub(super) serial: u64,
    pub(super) spp: Spp,
    pub(super) key_id: GroupKeyId,
    pub(super) activity: &'static Cell<u64>,
}
impl PartialEq for KeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial && core::ptr::eq(self.activity, other.activity)
    }
}
impl Eq for KeyIdentity {}
impl KeyIdentity {
    pub(super) fn active(&self) -> bool {
        self.activity.get() == self.serial
    }
}
impl KeyIdentity {
    pub fn role(&self) -> VifRole {
        self.role
    }
    pub fn key_id(&self) -> GroupKeyId {
        self.key_id
    }
}
pub struct PairwiseKey<'id, R> {
    pub(super) identity: KeyIdentity,
    pub(super) brand: PhantomData<fn(&'id ()) -> R>,
}
pub struct GroupKey<'id, R> {
    pub(super) identity: KeyIdentity,
    pub(super) brand: PhantomData<fn(&'id ()) -> R>,
}
pub struct PortKeys<R>(PhantomData<fn() -> R>);
pub(super) type StaPath<'id> = Child<CorePath<'id>, 2>;
pub(super) type ApPath<'id> = Child<CorePath<'id>, 3>;
pub(super) type StaPermission<'id> = Slot<'id, GroupPermission<Station>, Part<'id, 5>>;
pub(super) type ApPermission<'id> = Slot<'id, GroupPermission<AccessPoint>, Part<'id, 6>>;

macro_rules! keys {
    ($role:ty, $p:literal, $g:literal, $path:ty, $permission:ty) => {
        impl<'id> KeyTypes<'id> for PortKeys<$role> {
            type Pool = AssociationPool<$role, $p, $g>;
            type AssociationStart = AssociationStart<'id, $role, $p, $g, $path>;
            type Association = Association<'id, $role, $p, $g, $path>;
            type VacantKeyBank = VacantKeyBank<'id, $role, $p, $g, $path>;
            type VacantAssociation = VacantAssociation<'id, $role, $p, $g, $path>;
            type PairwiseSlot = PairwiseKeySlot<'id, $role, $p, $g, $path>;
            type GroupSlot = GroupKeySlot<'id, $role, $p, $g, $path>;
            type PairwiseKey = PairwiseKey<'id, $role>;
            type GroupKey = GroupKey<'id, $role>;
            type GroupPermission = $permission;
            type Identity = KeyIdentity;
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
            fn pairwise_identity(key: &Self::PairwiseKey) -> KeyIdentity {
                key.identity
            }
            fn group_identity(key: &Self::GroupKey) -> KeyIdentity {
                key.identity
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
keys!(Station, 1, 1, StaPath<'id>, StaPermission<'id>);
keys!(AccessPoint, 15, 3, ApPath<'id>, ApPermission<'id>);

pub(super) enum Installation<'id> {
    Transmit {
        spp: Spp,
        scope: TransmitKeyScope,
        slot: core_mac::association::VacantKeySlot<'id, CorePath<'id>>,
    },
    Receive {
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<'id>>,
        key_id: GroupKeyId,
    },
}
pub(super) enum Installed<'id> {
    Transmit(KeyHandle<'id, CorePath<'id>>),
    Receive(ReceiveKeyHandle<'id, CorePath<'id>>),
}
pub(super) enum Removal<'id> {
    StationPairwise(<PortKeys<Station> as KeyTypes<'id>>::Association),
    AccessPointPairwise(<PortKeys<AccessPoint> as KeyTypes<'id>>::Association),
    StationGroup(StaPermission<'id>),
    AccessPointGroup(ApPermission<'id>),
}
pub(super) enum GroupCommand<'id> {
    InstallStation {
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<'id>>,
        permission: StaPermission<'id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    InstallAccessPoint {
        spp: Spp,
        slot: core_mac::association::AccessPointGroupSlot<'id, CorePath<'id>>,
        permission: ApPermission<'id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    Replace {
        permission: StaPermission<'id>,
        key: GroupKey<'id, Station>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    },
    Select {
        permission: StaPermission<'id>,
        key: GroupKey<'id, Station>,
    },
    Remove {
        identity: KeyIdentity,
        removal: Removal<'id>,
    },
}

pub(super) trait Role<'id>: InterfaceRole + Sized
where
    PortKeys<Self>: KeyTypes<'id, Identity = KeyIdentity>,
{
    fn identity(
        association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
        slot: &<PortKeys<Self> as KeyTypes<'id>>::PairwiseSlot,
        serial: u64,
        activity: &'static [Cell<u64>; 20],
    ) -> KeyIdentity;
    fn pairwise(
        association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Self> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
    ) -> Installation<'id>;
    fn group(
        association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Self> as KeyTypes<'id>>::GroupSlot,
        permission: <PortKeys<Self> as KeyTypes<'id>>::GroupPermission,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id>;
    fn removal(association: <PortKeys<Self> as KeyTypes<'id>>::Association) -> Removal<'id>;
    fn remove_group(permission: <PortKeys<Self> as KeyTypes<'id>>::GroupPermission)
    -> Removal<'id>;
    fn pairwise_key(identity: KeyIdentity) -> <PortKeys<Self> as KeyTypes<'id>>::PairwiseKey;
    fn group_identity(key: <PortKeys<Self> as KeyTypes<'id>>::GroupKey) -> KeyIdentity;
}

macro_rules! role {
    ($role:ty, $p:literal, $g:literal, $path:ty, $install:ident, $remove:ident, $group_remove:ident) => {
        impl<'id> Role<'id> for $role {
            fn identity(
                association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
                slot: &<PortKeys<Self> as KeyTypes<'id>>::PairwiseSlot,
                serial: u64,
                activity: &'static [Cell<u64>; 20],
            ) -> KeyIdentity {
                KeyIdentity {
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
                association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
                slot: <PortKeys<Self> as KeyTypes<'id>>::PairwiseSlot,
                peer: IndividualAddress,
            ) -> Installation<'id> {
                let (spp, scope, slot) = core_mac::association::TransmitKeyInstallation::$install {
                    association,
                    slot,
                    peer: peer.get(),
                }
                .into_parts();
                Installation::Transmit { spp, scope, slot }
            }
            fn removal(
                association: <PortKeys<Self> as KeyTypes<'id>>::Association,
            ) -> Removal<'id> {
                Removal::$remove(association)
            }
            fn remove_group(
                permission: <PortKeys<Self> as KeyTypes<'id>>::GroupPermission,
            ) -> Removal<'id> {
                Removal::$group_remove(permission)
            }
            fn pairwise_key(identity: KeyIdentity) -> PairwiseKey<'id, Self> {
                PairwiseKey {
                    identity,
                    brand: PhantomData,
                }
            }
            fn group_identity(key: GroupKey<'id, Self>) -> KeyIdentity {
                key.identity
            }
            fn group(
                association: &<PortKeys<Self> as KeyTypes<'id>>::Association,
                slot: <PortKeys<Self> as KeyTypes<'id>>::GroupSlot,
                permission: <PortKeys<Self> as KeyTypes<'id>>::GroupPermission,
                key_id: GroupKeyId,
                material: Ccmp128Key,
            ) -> GroupCommand<'id> {
                Self::group_command(association.spp(), slot, permission, key_id, material)
            }
        }
    };
}
role!(
    Station,
    1,
    1,
    StaPath<'id>,
    StationPairwise,
    StationPairwise,
    StationGroup
);
role!(
    AccessPoint,
    15,
    3,
    ApPath<'id>,
    AccessPointPairwise,
    AccessPointPairwise,
    AccessPointGroup
);

trait GroupRole<'id>: InterfaceRole + Sized
where
    PortKeys<Self>: KeyTypes<'id>,
{
    fn group_command(
        spp: Spp,
        slot: <PortKeys<Self> as KeyTypes<'id>>::GroupSlot,
        permission: <PortKeys<Self> as KeyTypes<'id>>::GroupPermission,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id>;
}
impl<'id> GroupRole<'id> for Station {
    fn group_command(
        spp: Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<'id>>,
        permission: StaPermission<'id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id> {
        GroupCommand::InstallStation {
            spp,
            slot,
            permission,
            key_id,
            material,
        }
    }
}
impl<'id> GroupRole<'id> for AccessPoint {
    fn group_command(
        spp: Spp,
        slot: core_mac::association::AccessPointGroupSlot<'id, CorePath<'id>>,
        permission: ApPermission<'id>,
        key_id: GroupKeyId,
        material: Ccmp128Key,
    ) -> GroupCommand<'id> {
        GroupCommand::InstallAccessPoint {
            spp,
            slot,
            permission,
            key_id,
            material,
        }
    }
}
