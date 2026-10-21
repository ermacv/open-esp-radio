//! Whole model-owner conversions under the concrete frontend's private loan.

use super::*;
use oer_ieee80211_port::{BodyPayload, OwnedPayload};

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
        }
    }
}
impl<'p, B: ModelComposition> ModelAnyBuffer<'p, B> {
    pub(super) fn loan(&self) -> BodyLoan<'p, B::Identity, B> {
        let loan = match self {
            Self::Small(buffer) => buffer.loan,
            Self::Large(buffer) => buffer.loan,
            Self::SmallSent(sent) => sent.buffer.loan,
            Self::LargeSent(sent) => sent.buffer.loan,
        };
        #[expect(
            clippy::expect_used,
            reason = "only body resources can enter public preparation"
        )]
        loan.expect("only body resources can be prepared by a frontend")
    }
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
            packet_number: self.packet_number,
        }
    }
}
impl<'p, B: ModelComposition> BodyPayload<'p, B::Identity, B> for ModelSubframes<'p, B> {
    type Owned = ModelSubframes<'static, B>;
    fn into_owned(self, loan: &BodyLoan<'p, B::Identity, B>) -> Self::Owned {
        ModelSubframes(
            self.0
                .into_iter()
                .map(|subframe| Subframe {
                    mpdu: subframe.mpdu.into_owned(loan),
                    acknowledgement_bit: subframe.acknowledgement_bit,
                })
                .collect(),
        )
    }
}
impl<B: ModelComposition> OwnedPayload<B::Identity, B> for ModelSubframes<'static, B> {
    type Lent<'p>
        = ModelSubframes<'p, B>
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
        ModelSubframes(
            self.0
                .into_iter()
                .map(|subframe| Subframe {
                    mpdu: subframe.mpdu.lend(loan),
                    acknowledgement_bit: subframe.acknowledgement_bit,
                })
                .collect(),
        )
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
        ModelResources {
            station_association: self.station_association.lend(loan),
            access_point_association: self.access_point_association.lend(loan),
            station_commands: self.station_commands.lend(loan),
            access_point_commands: self.access_point_commands.lend(loan),
            connection_epoch: loan.lend_slot(self.connection_epoch),
            tickets: self
                .tickets
                .into_iter()
                .map(|slot| loan.lend_slot(slot))
                .collect(),
            terminal_access: self.terminal_access.lend(loan),
            small: self
                .small
                .into_iter()
                .map(|buffer| buffer.lend(loan))
                .collect(),
            large: self
                .large
                .into_iter()
                .map(|buffer| buffer.lend(loan))
                .collect(),
            station_pairwise: self.station_pairwise.map(|slot| slot.lend(loan)),
            station_group: self.station_group.map(|slot| slot.lend(loan)),
            access_point_pairwise: self.access_point_pairwise.map(|slot| slot.lend(loan)),
            access_point_group: self.access_point_group.map(|slot| slot.lend(loan)),
            block_acks: self.block_acks.map(|slot| slot.lend(loan)),
            station_group_permission: loan.lend_slot(self.station_group_permission),
            access_point_group_permission: loan.lend_slot(self.access_point_group_permission),
            gate: self.gate.lend(loan),
            arbiter_authority: self.arbiter_authority.lend(loan),
        }
    }
}
