//! Private queue payloads selected before the public body loan.

use super::*;
use oer_ieee80211_port::{
    Configured, InstalledGroup, LowerMacEvent, LowerMacTerminal, Removed, RemovedKey, TxCompleted,
};

pub(super) enum OwnedConfigured<'o, B: ModelComposition> {
    Station(
        ConfigurePermission<
            'o,
            Station,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 1>,
            ModelStationContextOf<B>,
        >,
    ),
    AccessPoint(
        ConfigurePermission<
            'o,
            AccessPoint,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 2>,
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
        permission: Slot<'o, ModelGroupPermission<R>, R::GroupPermissionPath<B::Identity>>,
    },
}

pub(super) enum OwnedRemoved<'o, B: ModelComposition> {
    Station(OwnedRemovedKey<'o, B, Station>),
    AccessPoint(OwnedRemovedKey<'o, B, AccessPoint>),
}

pub(super) enum OwnedInstalledGroup<'o, B: ModelComposition> {
    Station {
        key: ModelGroupKey<'o, B, Station>,
        permission: StationGroupPermission<'o, B>,
    },
    AccessPoint {
        key: ModelGroupKey<'o, B, AccessPoint>,
        permission: Slot<
            'o,
            ModelGroupPermission<AccessPoint>,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 0>,
        >,
    },
}

type SingleTerminal<'o, B> = oer_ieee80211_port::Completed<
    ModelContextOf<B>,
    Slot<'o, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>,
    ModelMpdu<'o, B>,
    ModelSingleOutcome,
>;
type AggregateTerminal<'o, B> = oer_ieee80211_port::Completed<
    ModelContextOf<B>,
    Slot<'o, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>,
    ModelSubframes<'o, B>,
    ModelAggregateOutcome,
>;
pub(super) enum OwnedTxCompleted<'o, B: ModelComposition> {
    Single(SingleTerminal<'o, B>),
    Aggregate(AggregateTerminal<'o, B>),
}

pub(super) enum OwnedSampledTsfPermission<'o, B: ModelComposition> {
    Station(
        oer_ieee80211_port::TsfPermission<
            'o,
            Station,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 1>,
        >,
    ),
    AccessPoint(
        oer_ieee80211_port::TsfPermission<
            'o,
            AccessPoint,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 2>,
        >,
    ),
}

pub(super) enum OwnedLowerMacTerminal<'o, B: ModelComposition> {
    TsfSampled {
        sample: oer_ieee80211_port::TsfSample,
        permission: OwnedSampledTsfPermission<'o, B>,
    },
    TxCompleted(OwnedTxCompleted<'o, B>),
    GateClosed(GateClosed<'o, ModelPool<B::Identity, 0>>),
    Configured(OwnedConfigured<'o, B>),
    KeyRemoved(OwnedRemoved<'o, B>),
    GroupKeyReplaced {
        key: ModelGroupKey<'o, B, Station>,
        permission: StationGroupPermission<'o, B>,
    },
    GroupKeyInstalled(OwnedInstalledGroup<'o, B>),
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

impl<B: ModelComposition> OwnedConfigured<'_, B> {
    pub(super) fn role(&self) -> VifRole {
        match self {
            Self::Station(_) => VifRole::Station,
            Self::AccessPoint(_) => VifRole::AccessPoint,
        }
    }
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
    pub(super) fn retain<'p>(
        context: Configured<'p, ModelTypes<B>>,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Self
    where
        B: 'p,
    {
        match context {
            Configured::Station(permission) => Self::Station(permission.into_owned(loan)),
            Configured::AccessPoint(permission) => Self::AccessPoint(permission.into_owned(loan)),
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
                permission: loan.lend_slot(permission),
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
                permission: loan.reclaim_slot(permission),
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
                permission: loan.lend_slot(permission),
            },
            Self::AccessPoint { key, permission } => InstalledGroup::AccessPoint {
                key: key.lend(loan),
                permission: loan.lend_slot(permission),
            },
        }
    }
    fn retain<'p>(
        key: InstalledGroup<'p, ModelTypes<B>>,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Self
    where
        B: 'p,
    {
        match key {
            InstalledGroup::Station { key, permission } => Self::Station {
                key: key.into_owned(loan),
                permission: loan.reclaim_slot(permission),
            },
            InstalledGroup::AccessPoint { key, permission } => Self::AccessPoint {
                key: key.into_owned(loan),
                permission: loan.reclaim_slot(permission),
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
            Self::TsfSampled { sample, permission } => LowerMacTerminal::TsfSampled {
                sample,
                permission: match permission {
                    OwnedSampledTsfPermission::Station(permission) => {
                        oer_ieee80211_port::SampledTsfPermission::Station(permission.lend(loan))
                    }
                    OwnedSampledTsfPermission::AccessPoint(permission) => {
                        oer_ieee80211_port::SampledTsfPermission::AccessPoint(permission.lend(loan))
                    }
                },
            },
            Self::TxCompleted(OwnedTxCompleted::Single(done)) => {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(done.lend(loan)))
            }
            Self::TxCompleted(OwnedTxCompleted::Aggregate(done)) => {
                LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(done.lend(loan)))
            }
            Self::GateClosed(gate) => LowerMacTerminal::GateClosed(gate.lend(loan)),
            Self::Configured(context) => LowerMacTerminal::Configured(context.lend(loan)),
            Self::KeyRemoved(key) => LowerMacTerminal::KeyRemoved(key.lend(loan)),
            Self::GroupKeyReplaced { key, permission } => LowerMacTerminal::GroupKeyReplaced {
                key: key.lend(loan),
                permission: loan.lend_slot(permission),
            },
            Self::GroupKeySelected { key, permission } => LowerMacTerminal::GroupKeySelected {
                key: key.lend(loan),
                permission: loan.lend_slot(permission),
            },
            Self::GroupKeyInstalled(key) => LowerMacTerminal::GroupKeyInstalled(key.lend(loan)),
        }
    }
    pub(super) fn retain<'p>(
        terminal: LowerMacTerminal<'p, ModelTypes<B>>,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> Self
    where
        B: 'p,
    {
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(done)) => {
                Self::TxCompleted(OwnedTxCompleted::Single(done.into_owned(loan)))
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(done)) => {
                Self::TxCompleted(OwnedTxCompleted::Aggregate(done.into_owned(loan)))
            }
            LowerMacTerminal::GateClosed(gate) => Self::GateClosed(gate.into_owned(loan)),
            LowerMacTerminal::Configured(context) => {
                Self::Configured(OwnedConfigured::retain(context, loan))
            }
            LowerMacTerminal::KeyRemoved(key) => Self::KeyRemoved(OwnedRemoved::retain(key, loan)),
            LowerMacTerminal::GroupKeyReplaced { key, permission } => Self::GroupKeyReplaced {
                key: key.into_owned(loan),
                permission: loan.reclaim_slot(permission),
            },
            LowerMacTerminal::GroupKeySelected { key, permission } => Self::GroupKeySelected {
                key: key.into_owned(loan),
                permission: loan.reclaim_slot(permission),
            },
            LowerMacTerminal::GroupKeyInstalled(key) => {
                Self::GroupKeyInstalled(OwnedInstalledGroup::retain(key, loan))
            }
            LowerMacTerminal::TsfSampled { sample, permission } => Self::TsfSampled {
                sample,
                permission: match permission {
                    oer_ieee80211_port::SampledTsfPermission::Station(permission) => {
                        OwnedSampledTsfPermission::Station(permission.into_owned(loan))
                    }
                    oer_ieee80211_port::SampledTsfPermission::AccessPoint(permission) => {
                        OwnedSampledTsfPermission::AccessPoint(permission.into_owned(loan))
                    }
                },
            },
        }
    }
}

impl<B: ModelComposition> OwnedLowerMacEvent<'static, B> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> LowerMacEvent<'p, ModelTypes<B>>
    where
        B: 'p,
    {
        match self {
            Self::Terminal(terminal) => LowerMacEvent::Terminal(terminal.lend(loan)),
            Self::Received { frame, meta } => LowerMacEvent::Received {
                frame: frame.lend(loan),
                meta,
            },
        }
    }
}
