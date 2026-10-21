//! Backend-only conversion between durable Id tokens and public body views.

use core::marker::PhantomData;

use super::{Brand, Line, Permit, PoolPath, Seat, Slot};
use crate::{SlotIdentity, SlotOwner};

/// Authority supplied only to a backend's body-view constructor.
///
/// Keep this value private in backend view implementations. It neither mints
/// nor copies capacity: conversions consume a token and preserve its identity,
/// component path, kind and index. No reference or payload is converted. The
/// concrete backend tag prevents a wrapper from forwarding its authority to
/// another backend's constructor. A body receives only the resulting views,
/// whose invariant loan prevents escape even when Id is static.
pub struct BodyLoan<'p, Id: SlotIdentity, S: SlotOwner<Id>> {
    brand: Brand<'p>,
    owner: PhantomData<fn(Id, S) -> (Id, S)>,
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> Copy for BodyLoan<'_, Id, S> {}
impl<Id: SlotIdentity, S: SlotOwner<Id>> Clone for BodyLoan<'_, Id, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'p, Id: SlotIdentity, S: SlotOwner<Id>> BodyLoan<'p, Id, S> {
    pub(crate) const fn new() -> Self {
        Self {
            brand: PhantomData,
            owner: PhantomData,
        }
    }

    /// Move one owned slot into its public body loan.
    pub fn lend_slot<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Slot<'static, K, P>,
    ) -> Slot<'p, K, P> {
        Slot {
            index: token.index,
            brand: self.brand,
            kind: token.kind,
        }
    }

    /// Move a returned body slot into owned queue or held state.
    pub fn reclaim_slot<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Slot<'p, K, P>,
    ) -> Slot<'static, K, P> {
        Slot {
            index: token.index,
            brand: PhantomData,
            kind: token.kind,
        }
    }

    /// Move one owned permit into its public body loan.
    pub fn lend_permit<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Permit<'static, K, P>,
    ) -> Permit<'p, K, P> {
        Permit {
            index: token.index,
            brand: self.brand,
            kind: token.kind,
        }
    }

    /// Return a permit to owned queue or pool state.
    pub fn reclaim_permit<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Permit<'p, K, P>,
    ) -> Permit<'static, K, P> {
        Permit {
            index: token.index,
            brand: PhantomData,
            kind: token.kind,
        }
    }

    /// Move one owned waiter seat into its public body loan.
    pub fn lend_seat<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Seat<'static, K, P>,
    ) -> Seat<'p, K, P> {
        Seat {
            index: token.index,
            brand: self.brand,
            kind: token.kind,
        }
    }

    /// Return a waiter seat to its owned installation state.
    pub fn reclaim_seat<K, P: PoolPath<Identity = Id>>(
        &self,
        token: Seat<'p, K, P>,
    ) -> Seat<'static, K, P> {
        Seat {
            index: token.index,
            brand: PhantomData,
            kind: token.kind,
        }
    }

    /// Lend the same waiter domain without changing its identity or size.
    pub fn lend_line<K, const N: usize, P: PoolPath<Identity = Id>>(
        &self,
        token: Line<'static, K, N, P>,
    ) -> Line<'p, K, N, P> {
        Line {
            brand: self.brand,
            kind: token.kind,
        }
    }

    /// Retain the same waiter domain in owned installation state.
    pub fn reclaim_line<K, const N: usize, P: PoolPath<Identity = Id>>(
        &self,
        token: Line<'p, K, N, P>,
    ) -> Line<'static, K, N, P> {
        Line {
            brand: PhantomData,
            kind: token.kind,
        }
    }
}
