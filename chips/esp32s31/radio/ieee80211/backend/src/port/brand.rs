//! The driver keeps an installation's owners under the `'static` brand;
//! the port lends them to its caller under the installation's brand and
//! takes them back. These are those conversions, and the `'static` forms
//! of the terminals the driver builds.
use super::*;
use keys::{
    AccessPointGroupInstallation, ApPermission, Installation, StaPermission,
    StationGroupInstallation,
};

impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Buffer<'p, Id, D, B, O> {
    pub(super) fn into_owned(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Buffer<'static, Id, D, B, O> {
        Buffer {
            place: loan.reclaim_slot(self.place),
            dma: self.dma,
            admission: self.admission,
            loan: None,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Buffer<'static, Id, D, B, O> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Buffer<'p, Id, D, B, O> {
        Buffer {
            place: loan.lend_slot(self.place),
            dma: self.dma,
            admission: self.admission,
            loan: Some(*loan),
        }
    }
}
impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    BodyPayload<'p, Id, Esp32s31Backend<Id, D, B, O>> for Mpdu<'p, Id, D, B, O>
{
    type Owned = Mpdu<'static, Id, D, B, O>;
    fn into_owned(self, loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>) -> Self::Owned {
        Mpdu {
            buffer: self.buffer.into_owned(loan),
            layout: self.layout,
            body: self.body,
            key: self.key,
            pn: self.pn,
            role: self.role,
            published: self.published,
            retry: self.retry,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    OwnedPayload<Id, Esp32s31Backend<Id, D, B, O>> for Mpdu<'static, Id, D, B, O>
{
    type Lent<'p>
        = Mpdu<'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B, O>: 'p;
    fn lend<'p>(self, loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>) -> Self::Lent<'p>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B, O>: 'p,
    {
        Mpdu {
            buffer: self.buffer.lend(loan),
            layout: self.layout,
            body: self.body,
            key: self.key,
            pn: self.pn,
            role: self.role,
            published: self.published,
            retry: self.retry,
        }
    }
}
impl<Id: SlotIdentity, R> PairwiseKey<'static, Id, R> {
    pub(super) fn lend<'p>(self) -> PairwiseKey<'p, Id, R> {
        PairwiseKey {
            identity: self.identity,
            brand: PhantomData,
        }
    }
}
#[cfg(test)]
impl<Id: SlotIdentity, R> GroupKey<'_, Id, R> {
    /// The installation's serial, distinct for every installation.
    pub(crate) fn serial(&self) -> u64 {
        self.identity.serial
    }
}
impl<'p, Id: SlotIdentity, R> GroupKey<'p, Id, R> {
    pub(super) fn into_owned(self) -> GroupKey<'static, Id, R> {
        GroupKey {
            identity: self.identity,
            brand: PhantomData,
        }
    }
}
impl<Id: SlotIdentity, R> GroupKey<'static, Id, R> {
    pub(super) fn lend<'p>(self) -> GroupKey<'p, Id, R> {
        GroupKey {
            identity: self.identity,
            brand: PhantomData,
        }
    }
}
impl<Id: SlotIdentity, R> ReceiveBuffer<'static, Id, R> {
    pub(super) fn lend<'p>(self) -> ReceiveBuffer<'p, Id, R> {
        self.rebrand()
    }
}
impl<'p, Id: SlotIdentity> Installation<'p, Id> {
    pub(super) fn into_owned<D: Device<B>, const B: usize, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Installation<'static, Id> {
        use core_mac::association::VacantKeySlot;
        match self {
            Self::Transmit { spp, scope, slot } => Installation::Transmit {
                spp,
                scope,
                slot: match slot {
                    VacantKeySlot::StationPairwise(s) => {
                        VacantKeySlot::StationPairwise(s.into_owned(loan))
                    }
                    VacantKeySlot::StationGroup(s) => {
                        VacantKeySlot::StationGroup(s.into_owned(loan))
                    }
                    VacantKeySlot::AccessPointPairwise(s) => {
                        VacantKeySlot::AccessPointPairwise(s.into_owned(loan))
                    }
                    VacantKeySlot::AccessPointGroup(s) => {
                        VacantKeySlot::AccessPointGroup(s.into_owned(loan))
                    }
                },
            },
            Self::Receive { spp, slot, key_id } => Installation::Receive {
                spp,
                slot: slot.into_owned(loan),
                key_id,
            },
        }
    }
}
macro_rules! group_installation {
    ($installation:ident) => {
        impl<'p, Id: SlotIdentity> $installation<'p, Id> {
            pub(super) fn into_owned<D: Device<B>, const B: usize, O: Observations>(
                self,
                loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
            ) -> $installation<'static, Id> {
                keys::GroupInstallation {
                    spp: self.spp,
                    slot: self.slot.into_owned(loan),
                    permission: self.permission.into_owned(loan),
                    key_id: self.key_id,
                    material: self.material,
                }
            }
        }
    };
}
group_installation!(StationGroupInstallation);
group_installation!(AccessPointGroupInstallation);
pub(super) enum OwnedConfigured<'o, Id: SlotIdentity, D: Device<B>, const B: usize> {
    Station(ConfigurePermission<'o, Station, Part<Id, 8>, D::OwnedConfigureContext<Id>>),
    AccessPoint(ConfigurePermission<'o, AccessPoint, Part<Id, 9>, D::OwnedConfigureContext<Id>>),
}
pub(super) enum RemovedOwners<S, A, G, P> {
    Pairwise { slot: S, association: A },
    Group { slot: G, permission: P },
}
pub(super) enum OwnedRemoved<'o, Id: SlotIdentity> {
    Station(
        RemovedOwners<
            association::PairwiseKeySlot<'o, Station, 1, 1, Child<CorePath<Id>, 2>>,
            association::Association<'o, Station, 1, 1, Child<CorePath<Id>, 2>>,
            association::GroupKeySlot<'o, Station, 1, 1, Child<CorePath<Id>, 2>>,
            StaPermission<'o, Id>,
        >,
    ),
    AccessPoint(
        RemovedOwners<
            association::PairwiseKeySlot<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            association::Association<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            association::GroupKeySlot<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            ApPermission<'o, Id>,
        >,
    ),
}
pub(super) enum OwnedInstalledGroup<'o, Id: SlotIdentity> {
    Station {
        key: GroupKey<'o, Id, Station>,
        permission: StaPermission<'o, Id>,
    },
    AccessPoint {
        key: GroupKey<'o, Id, AccessPoint>,
        permission: ApPermission<'o, Id>,
    },
}
pub(super) enum OwnedInstalledPairwise<'o, Id: SlotIdentity> {
    Station(PairwiseKey<'o, Id, Station>),
    AccessPoint(PairwiseKey<'o, Id, AccessPoint>),
}
impl<'o, Id: SlotIdentity> OwnedInstalledPairwise<'o, Id> {
    /// The installed owner of the identity the driver just activated, of its role.
    pub(super) fn new(role: VifRole, identity: super::keys::RawKeyIdentity) -> Self {
        match role {
            VifRole::Station => Self::Station(PairwiseKey {
                identity,
                brand: PhantomData,
            }),
            VifRole::AccessPoint => Self::AccessPoint(PairwiseKey {
                identity,
                brand: PhantomData,
            }),
        }
    }
}
pub(super) enum OwnedTxCompleted<
    'o,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    Single(OwnedSingle<'o, Id, D, B, O>),
    /// The access point's beacon and the beacon permission its admission
    /// took.
    Beacon {
        completed: OwnedSingle<'o, Id, D, B, O>,
        permission: BeaconPermission<'static, Part<Id, 10>>,
    },
}
/// A single MPDU's end with its owners, as the driver builds it.
pub(super) type OwnedSingle<'o, Id, D, const B: usize, O> = Completed<
    <D as Device<B>>::OwnedContext<Id>,
    Slot<'o, radio::Slots<Tickets, TX_TICKETS>, Part<Id, 2>>,
    Mpdu<'o, Id, D, B, O>,
    SingleOutcome<Ack, Infallible>,
>;
/// A published data arena's end, which carries its arena right and no
/// body-loaned owner.
pub(super) type ArenaDone =
    oer_protocols_ieee80211_contracts_port::arena::ArenaCompleted<BlockAckAnswer, Infallible>;
/// Every terminal but a transmission's, as the driver builds it.
pub(super) enum OwnedTerminal<'o, Id: SlotIdentity, D: Device<B>, const B: usize> {
    Configured(OwnedConfigured<'o, Id, D, B>),
    KeyRemoved(OwnedRemoved<'o, Id>),
    GroupKeyReplaced {
        key: GroupKey<'o, Id, Station>,
        permission: StaPermission<'o, Id>,
    },
    GroupKeySelected {
        key: GroupKey<'o, Id, Station>,
        permission: StaPermission<'o, Id>,
    },
    GroupKeyInstalled(OwnedInstalledGroup<'o, Id>),
    PairwiseKeyInstalled(OwnedInstalledPairwise<'o, Id>),
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> OwnedConfigured<'static, Id, D, B> {
    pub(super) fn lend<'p, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Configured<'p, Types<Id, D, B, O>>
    where
        Id: 'p,
    {
        match self {
            Self::Station(p) => Configured::Station(p.lend(loan)),
            Self::AccessPoint(p) => Configured::AccessPoint(p.lend(loan)),
        }
    }
}
impl<Id: SlotIdentity> OwnedRemoved<'static, Id> {
    fn lend<'p, D: Device<B>, const B: usize, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Removed<'p, Types<Id, D, B, O>>
    where
        Id: 'p,
    {
        match self {
            Self::Station(key) => Removed::Station(match key {
                RemovedOwners::Pairwise { slot, association } => RemovedKey::Pairwise {
                    slot: slot.lend(loan),
                    association: association.lend(loan),
                },
                RemovedOwners::Group { slot, permission } => RemovedKey::Group {
                    slot: slot.lend(loan),
                    permission: permission.lend(loan),
                },
            }),
            Self::AccessPoint(key) => Removed::AccessPoint(match key {
                RemovedOwners::Pairwise { slot, association } => RemovedKey::Pairwise {
                    slot: slot.lend(loan),
                    association: association.lend(loan),
                },
                RemovedOwners::Group { slot, permission } => RemovedKey::Group {
                    slot: slot.lend(loan),
                    permission: permission.lend(loan),
                },
            }),
        }
    }
}
impl<Id: SlotIdentity> OwnedInstalledGroup<'static, Id> {
    fn lend<'p, D: Device<B>, const B: usize, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> InstalledGroup<'p, Types<Id, D, B, O>>
    where
        Id: 'p,
    {
        match self {
            Self::Station { key, permission } => InstalledGroup::Station {
                key: key.lend(),
                permission: permission.lend(loan),
            },
            Self::AccessPoint { key, permission } => InstalledGroup::AccessPoint {
                key: key.lend(),
                permission: permission.lend(loan),
            },
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> OwnedTerminal<'static, Id, D, B> {
    pub(super) fn lend<'p, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Terminal<'p, Id, D, B, O>
    where
        Id: 'p,
    {
        match self {
            Self::Configured(c) => LowerMacTerminal::Configured(c.lend(loan)),
            Self::KeyRemoved(k) => LowerMacTerminal::KeyRemoved(k.lend(loan)),
            Self::GroupKeyReplaced { key, permission } => LowerMacTerminal::GroupKeyReplaced {
                key: key.lend(),
                permission: permission.lend(loan),
            },
            Self::GroupKeySelected { key, permission } => LowerMacTerminal::GroupKeySelected {
                key: key.lend(),
                permission: permission.lend(loan),
            },
            Self::GroupKeyInstalled(g) => LowerMacTerminal::GroupKeyInstalled(g.lend(loan)),
            Self::PairwiseKeyInstalled(OwnedInstalledPairwise::Station(key)) => {
                LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key.lend()))
            }
            Self::PairwiseKeyInstalled(OwnedInstalledPairwise::AccessPoint(key)) => {
                LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key.lend()))
            }
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    OwnedTxCompleted<'static, Id, D, B, O>
{
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Terminal<'p, Id, D, B, O>
    where
        Id: 'p,
    {
        match self {
            Self::Single(t) => LowerMacTerminal::TxCompleted(TxCompleted::Single(t.lend(loan))),
            Self::Beacon {
                completed,
                permission,
            } => LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed: completed.lend(loan),
                permission: permission.lend(loan),
            }),
        }
    }
}
