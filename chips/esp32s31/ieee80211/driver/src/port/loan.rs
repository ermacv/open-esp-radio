//! Concrete backend authority for whole owners crossing the body boundary.
use super::*;
use keys::{GroupCommand, Installation, Removal};

impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize> Buffer<'p, Id, D, B> {
    pub(super) fn into_owned(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
    ) -> Buffer<'static, Id, D, B> {
        Buffer {
            place: loan.reclaim_slot(self.place),
            dma: self.dma,
            admission: self.admission,
            loan: None,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Buffer<'static, Id, D, B> {
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
    ) -> Buffer<'p, Id, D, B> {
        Buffer {
            place: loan.lend_slot(self.place),
            dma: self.dma,
            admission: self.admission,
            loan: Some(*loan),
        }
    }
}
impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize>
    BodyPayload<'p, Id, Esp32s31Backend<Id, D, B>> for Mpdu<'p, Id, D, B>
{
    type Owned = Mpdu<'static, Id, D, B>;
    fn into_owned(self, loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>) -> Self::Owned {
        Mpdu {
            buffer: self.buffer.into_owned(loan),
            layout: self.layout,
            body: self.body,
            key: self.key,
            pn: self.pn,
            role: self.role,
            published: self.published,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> OwnedPayload<Id, Esp32s31Backend<Id, D, B>>
    for Mpdu<'static, Id, D, B>
{
    type Lent<'p>
        = Mpdu<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B>: 'p;
    fn lend<'p>(self, loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>) -> Self::Lent<'p>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B>: 'p,
    {
        Mpdu {
            buffer: self.buffer.lend(loan),
            layout: self.layout,
            body: self.body,
            key: self.key,
            pn: self.pn,
            role: self.role,
            published: self.published,
        }
    }
}
impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize>
    BodyPayload<'p, Id, Esp32s31Backend<Id, D, B>> for NoAggregatePayload<'p, Id, D, B>
{
    type Owned = NoAggregatePayload<'static, Id, D, B>;
    fn into_owned(self, _: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>) -> Self::Owned {
        match self.impossible {}
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> OwnedPayload<Id, Esp32s31Backend<Id, D, B>>
    for NoAggregatePayload<'static, Id, D, B>
{
    type Lent<'p>
        = NoAggregatePayload<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B>: 'p;
    fn lend<'p>(self, _: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>) -> Self::Lent<'p>
    where
        Self: 'p,
        Id: 'p,
        Esp32s31Backend<Id, D, B>: 'p,
    {
        match self.impossible {}
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
impl<'p, Id: SlotIdentity, R> ReceiveBuffer<'p, Id, R> {
    pub(super) fn into_owned(mut self) -> ReceiveBuffer<'static, Id, R> {
        ReceiveBuffer {
            unit: self.unit.take(),
            range: self.range.clone(),
            poison: self.poison,
            driver: self.driver,
            brand: PhantomData,
        }
    }
}
impl<Id: SlotIdentity, R> ReceiveBuffer<'static, Id, R> {
    pub(super) fn lend<'p>(mut self) -> ReceiveBuffer<'p, Id, R> {
        ReceiveBuffer {
            unit: self.unit.take(),
            range: self.range.clone(),
            poison: self.poison,
            driver: self.driver,
            brand: PhantomData,
        }
    }
}
impl<'p, Id: SlotIdentity> Installation<'p, Id> {
    pub(super) fn into_owned<D: Device<B>, const B: usize>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
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
impl<'p, Id: SlotIdentity> Removal<'p, Id> {
    pub(super) fn into_owned<D: Device<B>, const B: usize>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
    ) -> Removal<'static, Id> {
        match self {
            Self::StationPairwise(a) => Removal::StationPairwise(a.into_owned(loan)),
            Self::AccessPointPairwise(a) => Removal::AccessPointPairwise(a.into_owned(loan)),
            Self::StationGroup(p) => Removal::StationGroup(loan.reclaim_slot(p)),
            Self::AccessPointGroup(p) => Removal::AccessPointGroup(loan.reclaim_slot(p)),
        }
    }
}
impl<'p, Id: SlotIdentity> GroupCommand<'p, Id> {
    pub(super) fn into_owned<D: Device<B>, const B: usize>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
    ) -> GroupCommand<'static, Id> {
        match self {
            Self::InstallStation {
                spp,
                slot,
                permission,
                key_id,
                material,
            } => GroupCommand::InstallStation {
                spp,
                slot: slot.into_owned(loan),
                permission: loan.reclaim_slot(permission),
                key_id,
                material,
            },
            Self::InstallAccessPoint {
                spp,
                slot,
                permission,
                key_id,
                material,
            } => GroupCommand::InstallAccessPoint {
                spp,
                slot: slot.into_owned(loan),
                permission: loan.reclaim_slot(permission),
                key_id,
                material,
            },
            Self::Replace {
                permission,
                key,
                key_id,
                material,
            } => GroupCommand::Replace {
                permission: loan.reclaim_slot(permission),
                key: key.into_owned(),
                key_id,
                material,
            },
            Self::Select { permission, key } => GroupCommand::Select {
                permission: loan.reclaim_slot(permission),
                key: key.into_owned(),
            },
            Self::Remove { identity, removal } => GroupCommand::Remove {
                identity,
                removal: removal.into_owned(loan),
            },
        }
    }
}
