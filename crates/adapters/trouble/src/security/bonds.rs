//! Bounded bond storage for an authenticated application profile.

use core::{convert::Infallible, future::Future};
use trouble_host::{BondInformation, Identity, prelude::SecurityLevel};

/// Storage failures are distinct from pairing and radio failures.
#[derive(Debug, Eq, PartialEq)]
pub enum StoreError<E> {
    /// No unused slot; existing peers are never evicted automatically.
    Full,
    /// Replacing keys requires an explicit removal/re-enrollment operation.
    AlreadyStored,
    /// The record is unbonded or lacks authenticated encryption.
    InsufficientSecurity,
    /// Slot is outside the advertised capacity.
    InvalidSlot,
    /// The storage implementation failed; no durability is implied.
    Backend(E),
}

/// Semantic bond operations, not a flash, filesystem or key/value API.
///
/// A successful insert is available to subsequent reads. The backend determines
/// retention (RAM here), not this trait. Insert must not evict or overwrite a
/// record; errors leave existing records unchanged. The caller owns exclusive
/// mutation through `&mut self` and serializes Host restoration with enrollment.
///
/// A cancelled asynchronous write may have completed: reload before admitting
/// another enrollment or claiming success. No operation may block the radio
/// runner. Logical removal does not promise physical or compiler-proof erasure.
pub trait BondStore {
    type Error;

    /// Stable number of addressable slots; empty slots remain enumerable.
    fn capacity(&self) -> usize;

    fn load(
        &mut self,
        slot: usize,
    ) -> impl Future<Output = Result<Option<BondInformation>, StoreError<Self::Error>>>;

    fn insert(
        &mut self,
        bond: BondInformation,
    ) -> impl Future<Output = Result<(), StoreError<Self::Error>>>;

    /// Return whether a record was removed. Revoking a live connection and
    /// removing its Trouble working-set entry are separate caller obligations.
    fn remove(
        &mut self,
        identity: Identity,
    ) -> impl Future<Output = Result<bool, StoreError<Self::Error>>>;
}

/// Caller-owned volatile storage. No allocator, global owner or hardware access.
///
/// Deliberately not `Debug`: bond records contain secret key material.
pub struct RamBondStore<const N: usize> {
    slots: [Option<BondInformation>; N],
}

impl<const N: usize> RamBondStore<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; N],
        }
    }
}

impl<const N: usize> Default for RamBondStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> BondStore for RamBondStore<N> {
    type Error = Infallible;

    fn capacity(&self) -> usize {
        N
    }

    async fn load(
        &mut self,
        slot: usize,
    ) -> Result<Option<BondInformation>, StoreError<Self::Error>> {
        self.slots.get(slot).cloned().ok_or(StoreError::InvalidSlot)
    }

    async fn insert(&mut self, bond: BondInformation) -> Result<(), StoreError<Self::Error>> {
        if !bond.is_bonded || bond.security_level != SecurityLevel::EncryptedAuthenticated {
            return Err(StoreError::InsufficientSecurity);
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|old| old.identity.match_identity(&bond.identity))
        {
            return Err(StoreError::AlreadyStored);
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(StoreError::Full)?;
        *slot = Some(bond);
        Ok(())
    }

    async fn remove(&mut self, identity: Identity) -> Result<bool, StoreError<Self::Error>> {
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.as_ref()
                .is_some_and(|bond| bond.identity.match_identity(&identity))
        }) else {
            return Ok(false);
        };
        *slot = None;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
