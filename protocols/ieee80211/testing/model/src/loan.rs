//! Whole model-owner conversions under the concrete frontend's private loan.

use super::*;
use oer_protocols_ieee80211_contracts_port::{BodyPayload, OwnedPayload};

impl<'p, B: ModelComposition, C: ModelClass> ModelBuffer<'p, B, C> {
    fn into_owned(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelBuffer<'static, B, C> {
        ModelBuffer {
            slot: loan.reclaim_slot(self.slot),
            loan: None,
            memory: self.memory,
            admitted_at: self.admitted_at,
            admitted: self.admitted,
        }
    }
}
impl<B: ModelComposition, C: ModelClass> ModelBuffer<'static, B, C> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelBuffer<'p, B, C> {
        ModelBuffer {
            slot: loan.lend_slot(self.slot),
            loan: Some(*loan),
            memory: self.memory,
            admitted_at: self.admitted_at,
            admitted: self.admitted,
        }
    }
}
impl<'p, B: ModelComposition, C: ModelClass> ModelSent<'p, B, C> {
    fn into_owned(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelSent<'static, B, C> {
        ModelSent {
            role: self.role,
            buffer: self.buffer.into_owned(loan),
            layout: self.layout,
            response: self.response,
            key: self.key,
            body: self.body,
            packet_number: self.packet_number,
            retry: self.retry,
        }
    }
}
impl<B: ModelComposition, C: ModelClass> ModelSent<'static, B, C> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelSent<'p, B, C> {
        ModelSent {
            role: self.role,
            buffer: self.buffer.lend(loan),
            layout: self.layout,
            response: self.response,
            key: self.key,
            body: self.body,
            packet_number: self.packet_number,
            retry: self.retry,
        }
    }
}
impl<'p, B: ModelComposition> ModelAnyBuffer<'p, B> {
    fn into_owned(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelAnyBuffer<'static, B> {
        match self {
            Self::Small(buffer) => ModelAnyBuffer::Small(buffer.into_owned(loan)),
            Self::Large(buffer) => ModelAnyBuffer::Large(buffer.into_owned(loan)),
            Self::SmallSent(sent) => ModelAnyBuffer::SmallSent(sent.into_owned(loan)),
            Self::LargeSent(sent) => ModelAnyBuffer::LargeSent(sent.into_owned(loan)),
        }
    }
}
impl<B: ModelComposition> ModelAnyBuffer<'static, B> {
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelAnyBuffer<'p, B> {
        match self {
            Self::Small(buffer) => ModelAnyBuffer::Small(buffer.lend(loan)),
            Self::Large(buffer) => ModelAnyBuffer::Large(buffer.lend(loan)),
            Self::SmallSent(sent) => ModelAnyBuffer::SmallSent(sent.lend(loan)),
            Self::LargeSent(sent) => ModelAnyBuffer::LargeSent(sent.lend(loan)),
        }
    }
}
impl<'p, B: ModelComposition> BodyPayload<'p, B::Identity, B> for ModelMpdu<'p, B> {
    type Owned = ModelMpdu<'static, B>;
    fn into_owned(self, loan: &BodyLoan<'p, B::Identity, B>) -> Self::Owned {
        ModelMpdu {
            buffer: self.buffer.into_owned(loan),
            layout: self.layout,
            response: self.response,
            body: self.body,
            published: self.published,
            retry: self.retry,
            packet_number: self.packet_number,
        }
    }
}
impl<B: ModelComposition> OwnedPayload<B::Identity, B> for ModelMpdu<'static, B> {
    type Lent<'p>
        = ModelMpdu<'p, B>
    where
        Self: 'p,
        B::Identity: 'p,
        B: 'p;
    fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> Self::Lent<'p>
    where
        Self: 'p,
        B::Identity: 'p,
        B: 'p,
    {
        ModelMpdu {
            buffer: self.buffer.lend(loan),
            layout: self.layout,
            response: self.response,
            body: self.body,
            published: self.published,
            retry: self.retry,
            packet_number: self.packet_number,
        }
    }
}
impl<B: ModelComposition, R: ModelRole> ModelPairwiseKey<'static, B, R> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> ModelPairwiseKey<'p, B, R> {
        ModelPairwiseKey {
            identity: self.identity,
            slot: self.slot.lend(loan),
        }
    }
}
impl<'p, B: ModelComposition, R: ModelRole> ModelGroupKey<'p, B, R> {
    pub(super) fn into_owned(
        self,
        loan: &BodyLoan<'p, B::Identity, B>,
    ) -> ModelGroupKey<'static, B, R> {
        ModelGroupKey {
            identity: self.identity,
            slot: self.slot.into_owned(loan),
        }
    }
}
impl<B: ModelComposition, R: ModelRole> ModelGroupKey<'static, B, R> {
    pub(super) fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelGroupKey<'p, B, R> {
        ModelGroupKey {
            identity: self.identity,
            slot: self.slot.lend(loan),
        }
    }
}
impl<B: ModelComposition> ModelResources<'static, B> {
    pub(super) fn lend<'p>(self, loan: &BodyLoan<'p, B::Identity, B>) -> ModelResources<'p, B> {
        let Self {
            shared,
            station,
            access_point,
        } = self;
        ModelResources {
            shared: ModelShared {
                tickets: shared
                    .tickets
                    .into_iter()
                    .map(|slot| loan.lend_slot(slot))
                    .collect(),
                terminal_access: shared.terminal_access.lend(loan),
                small: shared
                    .small
                    .into_iter()
                    .map(|buffer| buffer.lend(loan))
                    .collect(),
                large: shared
                    .large
                    .into_iter()
                    .map(|buffer| buffer.lend(loan))
                    .collect(),
                block_acks: shared.block_acks.map(|slot| slot.lend(loan)),
                gate: shared.gate.lend(loan),
                arbiter_authority: shared.arbiter_authority.lend(loan),
            },
            station: ModelStationPermissions {
                association: station.association.lend(loan),
                commands: station.commands.lend(loan),
                connection_epoch: loan.lend_slot(station.connection_epoch),
                pairwise: station.pairwise.map(|slot| slot.lend(loan)),
                group: station.group.map(|slot| slot.lend(loan)),
                group_permission: loan.lend_slot(station.group_permission),
            },
            access_point: ModelAccessPointPermissions {
                association: access_point.association.lend(loan),
                commands: access_point.commands.lend(loan),
                pairwise: access_point.pairwise.map(|slot| slot.lend(loan)),
                group: access_point.group.map(|slot| slot.lend(loan)),
                group_permission: loan.lend_slot(access_point.group_permission),
                beacon: access_point.beacon.lend(loan),
            },
        }
    }
}
