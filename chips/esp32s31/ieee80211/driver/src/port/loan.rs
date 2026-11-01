//! Concrete backend authority for whole owners crossing the body boundary.
use super::*;
use keys::{GroupCommand, Installation, Removal};

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
        }
    }
}
impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    BodyPayload<'p, Id, Esp32s31Backend<Id, D, B, O>> for Subframes<'p, Id, D, B, O>
{
    type Owned = Subframes<'static, Id, D, B, O>;
    fn into_owned(self, loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>) -> Self::Owned {
        Subframes::into_owned(self, loan)
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    OwnedPayload<Id, Esp32s31Backend<Id, D, B, O>> for Subframes<'static, Id, D, B, O>
{
    type Lent<'p>
        = Subframes<'p, Id, D, B, O>
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
        Subframes::lend(self, loan)
    }
}
impl<'p, Id: SlotIdentity, R> PairwiseKey<'p, Id, R> {
    pub(super) fn into_owned(self) -> PairwiseKey<'static, Id, R> {
        PairwiseKey {
            identity: self.identity,
            brand: PhantomData,
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
impl<'p, Id: SlotIdentity, R> ReceiveBuffer<'p, Id, R> {
    pub(super) fn into_owned(self) -> ReceiveBuffer<'static, Id, R> {
        self.rebrand()
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
impl<'p, Id: SlotIdentity> Removal<'p, Id> {
    pub(super) fn into_owned<D: Device<B>, const B: usize, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
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
    pub(super) fn into_owned<D: Device<B>, const B: usize, O: Observations>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
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
