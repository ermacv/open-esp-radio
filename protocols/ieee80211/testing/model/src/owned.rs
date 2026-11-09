//! Private queue payloads selected before the public body loan.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Configured, InstalledGroup, InstalledPairwise, LowerMacTerminal, Removed, RemovedKey,
    TxCompleted,
};

pub(super) enum OwnedConfigured<'o, B: ModelComposition> {
    Station(
        ConfigurePermission<
            'o,
            Station,
            oer_protocols_radio_port::Child<ModelPool<B::Identity, 9>, 1>,
            ModelStationContextOf<B>,
        >,
    ),
    AccessPoint(
        ConfigurePermission<
            'o,
            AccessPoint,
            oer_protocols_radio_port::Child<ModelPool<B::Identity, 9>, 2>,
            ModelAccessPointContextOf<B>,
        >,
    ),
}

pub(super) enum OwnedRemovedKey<'o, B: ModelComposition, R: ModelRole> {
    Pairwise {
        slot: PairwiseKeySlot<
            'o,
            R,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            R::AssociationPath<B::Identity>,
        >,
        association: ModelAssociation<'o, B, R>,
    },
    Group {
        slot:
            GroupKeySlot<'o, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
        permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
            'o,
            oer_protocols_radio_port::Slots<ModelGroupPermission<R>, 1>,
            R::GroupPermissionPath<B::Identity>,
        >,
    },
}

pub(super) enum OwnedRemoved<'o, B: ModelComposition> {
    Station(OwnedRemovedKey<'o, B, Station>),
    AccessPoint(OwnedRemovedKey<'o, B, AccessPoint>),
}

pub(super) enum OwnedInstalledPairwise<'o, B: ModelComposition> {
    Station(ModelPairwiseKey<'o, B, Station>),
    AccessPoint(ModelPairwiseKey<'o, B, AccessPoint>),
}

impl<B: ModelComposition> OwnedInstalledPairwise<'static, B> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> InstalledPairwise<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::Station(key) => InstalledPairwise::Station(key.lend(loan)),
            Self::AccessPoint(key) => InstalledPairwise::AccessPoint(key.lend(loan)),
        }
    }
}

pub(super) enum OwnedInstalledGroup<'o, B: ModelComposition> {
    Station {
        key: ModelGroupKey<'o, B, Station>,
        permission: StationGroupPermission<'o, B>,
    },
    AccessPoint {
        key: ModelGroupKey<'o, B, AccessPoint>,
        permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
            'o,
            oer_protocols_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
            oer_protocols_radio_port::Child<ModelPool<B::Identity, 9>, 0>,
        >,
    },
}

type SingleTerminal<'o, B> = oer_protocols_ieee80211_contracts_port::Completed<
    ModelContextOf<B>,
    Slot<
        'o,
        oer_protocols_radio_port::Slots<ModelTickets, MODEL_TICKETS>,
        ModelPool<<B as ModelComposition>::Identity, 1>,
    >,
    ModelMpdu<'o, B>,
    ModelSingleOutcome,
>;
pub(super) enum OwnedTxCompleted<'o, B: ModelComposition> {
    Single(SingleTerminal<'o, B>),
    Beacon {
        completed: SingleTerminal<'o, B>,
        permission: OwnedBeacon<B>,
    },
}

impl<'o, B: ModelComposition> OwnedTxCompleted<'o, B> {
    /// A single MPDU's terminal, a beacon's when it held the permission.
    pub(super) fn of(completed: SingleTerminal<'o, B>, beacon: Option<OwnedBeacon<B>>) -> Self {
        match beacon {
            Some(permission) => Self::Beacon {
                completed,
                permission,
            },
            None => Self::Single(completed),
        }
    }
}

pub(super) enum OwnedLowerMacTerminal<'o, B: ModelComposition> {
    TxCompleted(OwnedTxCompleted<'o, B>),
    GateClosed(GateClosed<'o, ModelPool<B::Identity, 0>>),
    Configured(OwnedConfigured<'o, B>),
    KeyRemoved(OwnedRemoved<'o, B>),
    GroupKeyReplaced {
        key: ModelGroupKey<'o, B, Station>,
        permission: StationGroupPermission<'o, B>,
    },
    GroupKeyInstalled(OwnedInstalledGroup<'o, B>),
    PairwiseKeyInstalled(OwnedInstalledPairwise<'o, B>),
    GroupKeySelected {
        key: ModelGroupKey<'o, B, Station>,
        permission: StationGroupPermission<'o, B>,
    },
}

pub(super) enum OwnedLowerMacEvent<'o, B: ModelComposition> {
    Terminal(OwnedLowerMacTerminal<'o, B>),
    Received {
        frame: OwnedRxBuffer<B::Identity>,
        meta: ModelRxMeta,
    },
}

impl<B: ModelComposition> OwnedConfigured<'static, B> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Configured<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::Station(permission) => Configured::Station(permission.lend(loan)),
            Self::AccessPoint(permission) => Configured::AccessPoint(permission.lend(loan)),
        }
    }
}

impl<B: ModelComposition, R: ModelRole> OwnedRemovedKey<'static, B, R> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> RemovedKey<'p, ModelKeys<'p, B, R>>
    where
        B: 'p,
        R: 'p,
    {
        match self {
            Self::Pairwise { slot, association } => RemovedKey::Pairwise {
                slot: slot.lend(loan),
                association: ModelAssociation {
                    owner: association.owner.lend(loan),
                },
            },
            Self::Group { slot, permission } => RemovedKey::Group {
                slot: slot.lend(loan),
                permission: permission.lend(loan),
            },
        }
    }
    fn retain<'p>(
        key: RemovedKey<'p, ModelKeys<'p, B, R>>,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Self
    where
        B: 'p,
        R: 'p,
    {
        match key {
            RemovedKey::Pairwise { slot, association } => Self::Pairwise {
                slot: slot.into_owned(loan),
                association: ModelAssociation {
                    owner: association.owner.into_owned(loan),
                },
            },
            RemovedKey::Group { slot, permission } => Self::Group {
                slot: slot.into_owned(loan),
                permission: permission.into_owned(loan),
            },
        }
    }
}

impl<B: ModelComposition> OwnedRemoved<'static, B> {
    pub(super) fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> Removed<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::Station(key) => Removed::Station(key.lend(loan)),
            Self::AccessPoint(key) => Removed::AccessPoint(key.lend(loan)),
        }
    }
    pub(super) fn retain<'p>(
        key: Removed<'p, ModelTypes<B>>,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Self
    where
        B: 'p,
    {
        match key {
            Removed::Station(key) => Self::Station(OwnedRemovedKey::retain(key, loan)),
            Removed::AccessPoint(key) => Self::AccessPoint(OwnedRemovedKey::retain(key, loan)),
        }
    }
}

impl<B: ModelComposition> OwnedInstalledGroup<'static, B> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> InstalledGroup<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::Station { key, permission } => InstalledGroup::Station {
                key: key.lend(loan),
                permission: permission.lend(loan),
            },
            Self::AccessPoint { key, permission } => InstalledGroup::AccessPoint {
                key: key.lend(loan),
                permission: permission.lend(loan),
            },
        }
    }
}

impl<B: ModelComposition> OwnedLowerMacTerminal<'static, B> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> LowerMacTerminal<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::TxCompleted(OwnedTxCompleted::Single(done)) => {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(done.lend(loan)))
            }
            Self::TxCompleted(OwnedTxCompleted::Beacon {
                completed,
                permission,
            }) => LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed: completed.lend(loan),
                permission: permission.lend(loan),
            }),
            Self::GateClosed(gate) => LowerMacTerminal::GateClosed(gate.lend(loan)),
            Self::Configured(context) => LowerMacTerminal::Configured(context.lend(loan)),
            Self::KeyRemoved(key) => LowerMacTerminal::KeyRemoved(key.lend(loan)),
            Self::GroupKeyReplaced { key, permission } => LowerMacTerminal::GroupKeyReplaced {
                key: key.lend(loan),
                permission: permission.lend(loan),
            },
            Self::GroupKeySelected { key, permission } => LowerMacTerminal::GroupKeySelected {
                key: key.lend(loan),
                permission: permission.lend(loan),
            },
            Self::GroupKeyInstalled(key) => LowerMacTerminal::GroupKeyInstalled(key.lend(loan)),
            Self::PairwiseKeyInstalled(key) => {
                LowerMacTerminal::PairwiseKeyInstalled(key.lend(loan))
            }
        }
    }
}
