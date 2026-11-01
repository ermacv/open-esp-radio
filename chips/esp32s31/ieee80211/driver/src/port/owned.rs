//! Owned queue messages selected before any public body loan.
use super::*;
use keys::{ApPermission, StaPermission};
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
            Slot<'o, GroupPermission<Station>, Part<Id, 5>>,
        >,
    ),
    AccessPoint(
        RemovedOwners<
            association::PairwiseKeySlot<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            association::Association<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            association::GroupKeySlot<'o, AccessPoint, 15, 3, Child<CorePath<Id>, 3>>,
            Slot<'o, GroupPermission<AccessPoint>, Part<Id, 6>>,
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
    /// The installed owner of the identity P2 just activated, of its role.
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
pub(super) enum OwnedTsfPermission<'o, Id: SlotIdentity> {
    Station(TsfPermission<'o, Station, Part<Id, 8>>),
    AccessPoint(TsfPermission<'o, AccessPoint, Part<Id, 9>>),
}
pub(super) enum OwnedTxCompleted<
    'o,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    Single(
        Completed<
            D::OwnedContext<Id>,
            Slot<'o, Tickets, Part<Id, 2>>,
            Mpdu<'o, Id, D, B, O>,
            SingleOutcome<Ack, Infallible>,
        >,
    ),
    Aggregate(
        Completed<
            D::OwnedContext<Id>,
            Slot<'o, Tickets, Part<Id, 2>>,
            Subframes<'o, Id, D, B, O>,
            AggregateOutcome<BlockAckAnswer, Infallible>,
        >,
    ),
}
pub(super) enum OwnedTerminal<'o, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    TxCompleted(OwnedTxCompleted<'o, Id, D, B, O>),
    TsfSampled {
        sample: TsfSample,
        permission: OwnedTsfPermission<'o, Id>,
    },
    GateClosed(GateClosed<'o, Part<Id, 1>>),
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
pub(super) enum OwnedEvent<'o, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Terminal(OwnedTerminal<'o, Id, D, B, O>),
    Received {
        frame: ReceiveBuffer<'o, Id, D::Receive>,
        meta: Esp32s31RxMeta,
    },
    Tbtt(TbttEvent),
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
    pub(super) fn retain<'p, O: Observations>(
        p: Configured<'p, Types<Id, D, B, O>>,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Self
    where
        Id: 'p,
    {
        match p {
            Configured::Station(p) => Self::Station(p.into_owned(loan)),
            Configured::AccessPoint(p) => Self::AccessPoint(p.into_owned(loan)),
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
                    permission: loan.lend_slot(permission),
                },
            }),
            Self::AccessPoint(key) => Removed::AccessPoint(match key {
                RemovedOwners::Pairwise { slot, association } => RemovedKey::Pairwise {
                    slot: slot.lend(loan),
                    association: association.lend(loan),
                },
                RemovedOwners::Group { slot, permission } => RemovedKey::Group {
                    slot: slot.lend(loan),
                    permission: loan.lend_slot(permission),
                },
            }),
        }
    }
    fn retain<'p, D: Device<B>, const B: usize, O: Observations>(
        r: Removed<'p, Types<Id, D, B, O>>,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Self
    where
        Id: 'p,
    {
        match r {
            Removed::Station(key) => Self::Station(match key {
                RemovedKey::Pairwise { slot, association } => RemovedOwners::Pairwise {
                    slot: slot.into_owned(loan),
                    association: association.into_owned(loan),
                },
                RemovedKey::Group { slot, permission } => RemovedOwners::Group {
                    slot: slot.into_owned(loan),
                    permission: loan.reclaim_slot(permission),
                },
            }),
            Removed::AccessPoint(key) => Self::AccessPoint(match key {
                RemovedKey::Pairwise { slot, association } => RemovedOwners::Pairwise {
                    slot: slot.into_owned(loan),
                    association: association.into_owned(loan),
                },
                RemovedKey::Group { slot, permission } => RemovedOwners::Group {
                    slot: slot.into_owned(loan),
                    permission: loan.reclaim_slot(permission),
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
                permission: loan.lend_slot(permission),
            },
            Self::AccessPoint { key, permission } => InstalledGroup::AccessPoint {
                key: key.lend(),
                permission: loan.lend_slot(permission),
            },
        }
    }
    fn retain<'p, D: Device<B>, const B: usize, O: Observations>(
        g: InstalledGroup<'p, Types<Id, D, B, O>>,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Self
    where
        Id: 'p,
    {
        match g {
            InstalledGroup::Station { key, permission } => Self::Station {
                key: key.into_owned(),
                permission: loan.reclaim_slot(permission),
            },
            InstalledGroup::AccessPoint { key, permission } => Self::AccessPoint {
                key: key.into_owned(),
                permission: loan.reclaim_slot(permission),
            },
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    OwnedTerminal<'static, Id, D, B, O>
{
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Terminal<'p, Id, D, B, O>
    where
        Id: 'p,
    {
        match self {
            Self::TxCompleted(OwnedTxCompleted::Single(t)) => {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(t.lend(loan)))
            }
            Self::TxCompleted(OwnedTxCompleted::Aggregate(t)) => {
                LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(t.lend(loan)))
            }
            Self::TsfSampled { sample, permission } => LowerMacTerminal::TsfSampled {
                sample,
                permission: match permission {
                    OwnedTsfPermission::Station(p) => SampledTsfPermission::Station(p.lend(loan)),
                    OwnedTsfPermission::AccessPoint(p) => {
                        SampledTsfPermission::AccessPoint(p.lend(loan))
                    }
                },
            },
            Self::GateClosed(g) => LowerMacTerminal::GateClosed(g.lend(loan)),
            Self::Configured(c) => LowerMacTerminal::Configured(c.lend(loan)),
            Self::KeyRemoved(k) => LowerMacTerminal::KeyRemoved(k.lend(loan)),
            Self::GroupKeyReplaced { key, permission } => LowerMacTerminal::GroupKeyReplaced {
                key: key.lend(),
                permission: loan.lend_slot(permission),
            },
            Self::GroupKeySelected { key, permission } => LowerMacTerminal::GroupKeySelected {
                key: key.lend(),
                permission: loan.lend_slot(permission),
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
    pub(super) fn retain<'p>(
        t: Terminal<'p, Id, D, B, O>,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Self
    where
        Id: 'p,
    {
        match t {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(t)) => {
                Self::TxCompleted(OwnedTxCompleted::Single(t.into_owned(loan)))
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(t)) => {
                Self::TxCompleted(OwnedTxCompleted::Aggregate(t.into_owned(loan)))
            }
            LowerMacTerminal::TsfSampled { sample, permission } => Self::TsfSampled {
                sample,
                permission: match permission {
                    SampledTsfPermission::Station(p) => {
                        OwnedTsfPermission::Station(p.into_owned(loan))
                    }
                    SampledTsfPermission::AccessPoint(p) => {
                        OwnedTsfPermission::AccessPoint(p.into_owned(loan))
                    }
                },
            },
            LowerMacTerminal::GateClosed(g) => Self::GateClosed(g.into_owned(loan)),
            LowerMacTerminal::Configured(c) => Self::Configured(OwnedConfigured::retain(c, loan)),
            LowerMacTerminal::KeyRemoved(k) => Self::KeyRemoved(OwnedRemoved::retain(k, loan)),
            LowerMacTerminal::GroupKeyReplaced { key, permission } => Self::GroupKeyReplaced {
                key: key.into_owned(),
                permission: loan.reclaim_slot(permission),
            },
            LowerMacTerminal::GroupKeySelected { key, permission } => Self::GroupKeySelected {
                key: key.into_owned(),
                permission: loan.reclaim_slot(permission),
            },
            LowerMacTerminal::GroupKeyInstalled(g) => {
                Self::GroupKeyInstalled(OwnedInstalledGroup::retain(g, loan))
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)) => {
                Self::PairwiseKeyInstalled(OwnedInstalledPairwise::Station(key.into_owned()))
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                Self::PairwiseKeyInstalled(OwnedInstalledPairwise::AccessPoint(key.into_owned()))
            }
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    OwnedEvent<'static, Id, D, B, O>
{
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Event<'p, Id, D, B, O>
    where
        Id: 'p,
    {
        match self {
            Self::Terminal(t) => LowerMacEvent::Terminal(t.lend(loan)),
            Self::Received { frame, meta } => LowerMacEvent::Received {
                frame: frame.lend(),
                meta,
            },
            Self::Tbtt(t) => LowerMacEvent::Tbtt(t),
        }
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize> OwnedConfigured<'_, Id, D, B> {
    pub(super) fn role(&self) -> VifRole {
        match self {
            Self::Station(_) => VifRole::Station,
            Self::AccessPoint(_) => VifRole::AccessPoint,
        }
    }
}
