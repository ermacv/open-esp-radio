//! Nominal component identity inside an installation's pool layout.

use crate::SlotIdentity;
use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A path issued only by the pool factory. Paths occupy no storage.
///
/// The root carries the slot identity. Children distinguish components,
/// including nested components of the same kind, within that slot.
pub trait PoolPath: sealed::Sealed + Copy + core::fmt::Debug + Eq {
    /// The slot that owns this component of the pool layout.
    type Identity: SlotIdentity;
}

/// The root authority consumed by one installation's Mint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Root<Id: SlotIdentity>(PhantomData<fn(Id) -> Id>);

/// Component `INDEX` of a tuple built at `Parent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Child<Parent: PoolPath, const INDEX: u8>(PhantomData<fn() -> Parent>);

impl<Id: SlotIdentity> sealed::Sealed for Root<Id> {}
impl<Id: SlotIdentity> PoolPath for Root<Id> {
    type Identity = Id;
}
impl<P: PoolPath, const I: u8> sealed::Sealed for Child<P, I> {}
impl<P: PoolPath, const I: u8> PoolPath for Child<P, I> {
    type Identity = P::Identity;
}
