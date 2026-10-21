//! Backend-private loans of complete capacity owners; no capacity is minted.

use super::*;
use oer_radio_port::{BodyLoan, SlotOwner};

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    AssociationStart<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> AssociationStart<'p, R, P, G, Path> {
        AssociationStart {
            _owner: loan.lend_slot(self._owner),
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    AssociationStart<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> AssociationStart<'static, R, P, G, Path> {
        AssociationStart {
            _owner: loan.reclaim_slot(self._owner),
        }
    }
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    PairwiseKeySlot<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> PairwiseKeySlot<'p, R, P, G, Path> {
        PairwiseKeySlot {
            slot: loan.lend_slot(self.slot),
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    PairwiseKeySlot<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> PairwiseKeySlot<'static, R, P, G, Path> {
        PairwiseKeySlot {
            slot: loan.reclaim_slot(self.slot),
        }
    }
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    GroupKeySlot<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> GroupKeySlot<'p, R, P, G, Path> {
        GroupKeySlot {
            slot: loan.lend_slot(self.slot),
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    GroupKeySlot<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> GroupKeySlot<'static, R, P, G, Path> {
        GroupKeySlot {
            slot: loan.reclaim_slot(self.slot),
        }
    }
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    Association<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> Association<'p, R, P, G, Path> {
        Association {
            start: self.start.lend(loan),
            spp: self.spp,
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    Association<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> Association<'static, R, P, G, Path> {
        Association {
            start: self.start.into_owned(loan),
            spp: self.spp,
        }
    }
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantKeyBank<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> VacantKeyBank<'p, R, P, G, Path> {
        VacantKeyBank {
            pairwise: self.pairwise.map(|slot| slot.lend(loan)),
            group: self.group.map(|slot| slot.lend(loan)),
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantKeyBank<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> VacantKeyBank<'static, R, P, G, Path> {
        VacantKeyBank {
            pairwise: self.pairwise.map(|slot| slot.into_owned(loan)),
            group: self.group.map(|slot| slot.into_owned(loan)),
        }
    }
}

impl<R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantAssociation<'static, R, P, G, Path>
{
    /// Lend the same complete owner to a body.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> VacantAssociation<'p, R, P, G, Path> {
        VacantAssociation {
            start: self.start.lend(loan),
            bank: self.bank.lend(loan),
        }
    }
}

impl<'p, R: InterfaceRole, const P: usize, const G: usize, Path: PoolPath>
    VacantAssociation<'p, R, P, G, Path>
{
    /// Retain the complete owner in backend queue custody.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> VacantAssociation<'static, R, P, G, Path> {
        VacantAssociation {
            start: self.start.into_owned(loan),
            bank: self.bank.into_owned(loan),
        }
    }
}

impl<K, const N: usize, Path: PoolPath> BlockAckSlot<'static, K, N, Path> {
    /// Move the same slot across its backend's body boundary.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> BlockAckSlot<'p, K, N, Path> {
        BlockAckSlot {
            slot: loan.lend_slot(self.slot),
        }
    }
}

impl<'p, K, const N: usize, Path: PoolPath> BlockAckSlot<'p, K, N, Path> {
    /// Move the same slot across its backend's body boundary.
    pub fn into_owned<S: SlotOwner<Path::Identity>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> BlockAckSlot<'static, K, N, Path> {
        BlockAckSlot {
            slot: loan.reclaim_slot(self.slot),
        }
    }
}
