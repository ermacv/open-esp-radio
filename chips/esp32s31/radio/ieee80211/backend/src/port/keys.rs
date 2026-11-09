use super::*;
use oer_protocols_ieee80211_contracts_port::association::{self as association, *};
use oer_protocols_ieee80211_wire::security::Spp;

#[derive(Clone, Copy, Debug)]
pub struct RawKeyIdentity {
    pub(super) role: VifRole,
    pub(super) slot: u8,
    pub(super) serial: u64,
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

    /// The generation of the installation that issued the key: the high
    /// word of its serial ([`Serials`](super::engines)).
    pub(super) fn generation(&self) -> u32 {
        let [a, b, c, d, ..] = self.serial.to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }
}
/// A read-only identity bound to both the slot and its installation's brand.
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
#[cfg(test)]
impl<Id: SlotIdentity, R> PairwiseKey<'_, Id, R> {
    /// The generation of the installation that issued the key.
    pub(crate) fn generation(&self) -> u32 {
        self.identity.generation()
    }
}
#[cfg(test)]
impl<Id: SlotIdentity, R> GroupKey<'_, Id, R> {
    /// The generation of the installation that issued the key.
    pub(crate) fn generation(&self) -> u32 {
        self.identity.generation()
    }
}
pub struct PortKeys<Id: SlotIdentity, R>(PhantomData<fn(Id) -> (Id, R)>);
pub(super) type StaPath<Id: SlotIdentity> = Child<CorePath<Id>, 2>;
pub(super) type ApPath<Id: SlotIdentity> = Child<CorePath<Id>, 3>;
pub(super) type StaPermission<'id, Id: SlotIdentity> =
    IssuedSlot<'id, GroupPermission<Station>, Part<Id, 5>>;
pub(super) type ApPermission<'id, Id: SlotIdentity> =
    IssuedSlot<'id, GroupPermission<AccessPoint>, Part<Id, 6>>;

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
                // The pool's geometry is fixed at compile time: the station's
                // PTK slot is the first, and `from_array` checks that the
                // spare slots fit the bank's capacity during monomorphization.
                let [first, spare @ ..] = pairwise;
                let parts = KeyBankParts {
                    pairwise: heapless::Vec::from_array(spare),
                    group: heapless::Vec::from_array(group),
                };
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
/// One place of the driver's key table: an installed key, or the removal
/// its command left there while the key's last use is out.
pub(super) enum KeyPlace<'id, Id: SlotIdentity> {
    Installed(RawKeyIdentity, Installed<'id, Id>),
    Removing(WaitingRemoval<'id, Id>),
}
impl<'id, Id: SlotIdentity> KeyPlace<'id, Id> {
    /// The installed key this place holds.
    pub(super) fn installed(&self) -> Option<(&RawKeyIdentity, &Installed<'id, Id>)> {
        match self {
            Self::Installed(identity, installed) => Some((identity, installed)),
            Self::Removing(_) => None,
        }
    }
    /// The installed key this place holds, to change.
    pub(super) fn installed_mut(
        &mut self,
    ) -> Option<(&mut RawKeyIdentity, &mut Installed<'id, Id>)> {
        match self {
            Self::Installed(identity, installed) => Some((identity, installed)),
            Self::Removing(_) => None,
        }
    }
}
/// What a removal's command left: the receipt now, or the core place where
/// it finishes once the key's last use returns, both of the slot kind its
/// rights `R` take. Either way it holds the key's table place, which this
/// removal emptied: the only place its command's rights can wait in, and no
/// other removal can name it.
pub(super) struct Removing<'a, 'id, Id: SlotIdentity, R: RemovalRights<'id, Id>> {
    pub(super) place: &'a mut Option<KeyPlace<'id, Id>>,
    pub(super) ended: Removed<'id, Id, R::Slot>,
}
pub(super) enum Removed<'id, Id: SlotIdentity, S> {
    Now(core_mac::KeyRemoved<'id, CorePath<Id>, S>),
    Waiting(core_mac::RemovalPlace<'id, CorePath<Id>, S>),
}
/// A removal its command left waiting in its key's table place: the core
/// place where it finishes and the command's rights, which its terminal
/// returns.
pub(super) struct Waiting<'id, Id: SlotIdentity, R: RemovalRights<'id, Id>> {
    pub(super) core: core_mac::RemovalPlace<'id, CorePath<Id>, R::Slot>,
    pub(super) rights: R,
}
/// A waiting removal, of each kind of rights.
pub(super) enum WaitingRemoval<'id, Id: SlotIdentity> {
    StationPairwise(Waiting<'id, Id, core_mac::association::StationAssociation<'id, CorePath<Id>>>),
    AccessPointPairwise(
        Waiting<'id, Id, core_mac::association::AccessPointAssociation<'id, CorePath<Id>>>,
    ),
    StationGroup(Waiting<'id, Id, StaPermission<'id, Id>>),
    AccessPointGroup(Waiting<'id, Id, ApPermission<'id, Id>>),
}
/// A removal command's rights: the slot kind whose receipt they take, and
/// the terminal the receipt's slot ends with them in.
pub(super) trait RemovalRights<'id, Id: SlotIdentity>: Sized {
    /// The slot kind of the key these rights remove.
    type Slot: core_mac::RemovableSlot<'id, CorePath<Id>>;
    /// The terminal of the removal: the receipt's slot with these rights.
    fn terminal(self, slot: Self::Slot) -> brand::OwnedRemoved<'id, Id>;
    /// These rights waiting at `core` for the receipt.
    fn waiting(waiting: Waiting<'id, Id, Self>) -> WaitingRemoval<'id, Id>;
}
macro_rules! rights {
    ($rights:ty, $slot:ident, $waiting:ident, $role:ident, $owners:ident, $field:ident) => {
        impl<'id, Id: SlotIdentity> RemovalRights<'id, Id> for $rights {
            type Slot = core_mac::association::$slot<'id, CorePath<Id>>;
            fn terminal(self, slot: Self::Slot) -> brand::OwnedRemoved<'id, Id> {
                brand::OwnedRemoved::$role(brand::RemovedOwners::$owners { slot, $field: self })
            }
            fn waiting(waiting: Waiting<'id, Id, Self>) -> WaitingRemoval<'id, Id> {
                WaitingRemoval::$waiting(waiting)
            }
        }
    };
}
rights!(
    core_mac::association::StationAssociation<'id, CorePath<Id>>,
    StationPairwiseSlot,
    StationPairwise,
    Station,
    Pairwise,
    association
);
rights!(
    core_mac::association::AccessPointAssociation<'id, CorePath<Id>>,
    AccessPointPairwiseSlot,
    AccessPointPairwise,
    AccessPoint,
    Pairwise,
    association
);
rights!(
    StaPermission<'id, Id>,
    StationGroupSlot,
    StationGroup,
    Station,
    Group,
    permission
);
rights!(
    ApPermission<'id, Id>,
    AccessPointGroupSlot,
    AccessPointGroup,
    AccessPoint,
    Group,
    permission
);
impl<Id: SlotIdentity> Installed<'_, Id> {
    /// The kind of slot the installed key holds.
    pub(super) const fn kind(&self) -> core_mac::association::KeySlotKind {
        match self {
            Self::Transmit(key) => key.slot_kind(),
            Self::Receive(key) => key.slot_kind(),
        }
    }
}
/// A group key installation of one role: the role's group slot and
/// permission, with the key.
pub(super) struct GroupInstallation<S, P> {
    pub(super) spp: Spp,
    pub(super) slot: S,
    pub(super) permission: P,
    pub(super) key_id: GroupKeyId,
    pub(super) material: Ccmp128Key,
}
/// The station's group key installation.
pub(super) type StationGroupInstallation<'id, Id> = GroupInstallation<
    core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
    StaPermission<'id, Id>,
>;
/// The access point's group key installation.
pub(super) type AccessPointGroupInstallation<'id, Id> = GroupInstallation<
    core_mac::association::AccessPointGroupSlot<'id, CorePath<Id>>,
    ApPermission<'id, Id>,
>;
/// A station group key mutation, with its owners.
pub(super) enum StationGroupCommand<'id, Id: SlotIdentity> {
    Install(StationGroupInstallation<'id, Id>),
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
        permission: StaPermission<'id, Id>,
        key: GroupKey<'id, Id, Station>,
    },
}
/// An access-point group key mutation, with its owners.
pub(super) enum AccessPointGroupCommand<'id, Id: SlotIdentity> {
    Install(AccessPointGroupInstallation<'id, Id>),
    Remove {
        permission: ApPermission<'id, Id>,
        key: GroupKey<'id, Id, AccessPoint>,
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
}

macro_rules! role {
    ($role:ty, $install:ident) => {
        impl<'id, Id: SlotIdentity + 'id> Role<'id, Id> for $role {
            fn identity(
                _association: &<PortKeys<Id, Self> as KeyTypes<'id>>::Association,
                slot: &<PortKeys<Id, Self> as KeyTypes<'id>>::PairwiseSlot,
                serial: u64,
                activity: &'static [KeyActivity; 20],
            ) -> RawKeyIdentity {
                RawKeyIdentity {
                    role: Self::ROLE,
                    slot: slot.index(),
                    serial,
                    key_id: const { GroupKeyId::new(0).expect("wire KeyID zero") },
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
        }
    };
}
role!(Station, StationPairwise);
role!(AccessPoint, AccessPointPairwise);
