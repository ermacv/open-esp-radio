//! Closed identities of caller-owned port storage.

use core::{fmt, marker::PhantomData};

mod sealed {
    pub trait Sealed {}
}

/// The identity of one caller-owned slot, independent of a body loan.
///
/// Only a generative [`LocalId`] or a nominal [`StaticId`] implements this
/// trait. An identity is a type marker; it is not construction or reuse
/// authority for the storage bearing that identity.
pub trait SlotIdentity: sealed::Sealed + Copy + Eq + fmt::Debug {}

/// A generative identity belonging to an outer local execution scope.
///
/// The scope is invariant: a shorter borrow cannot substitute another
/// scope's identity. The marker carries no global latch or mutable state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalId<'scope>(PhantomData<fn(&'scope ()) -> &'scope ()>);

/// The nominal identity of a static slot declaration.
///
/// The declaration type is invariant. Naming this type grants neither
/// construction authority nor permission to take the declaration twice.
pub struct StaticId<Declaration>(PhantomData<fn(Declaration) -> Declaration>);

impl<D> Copy for StaticId<D> {}
impl<D> Clone for StaticId<D> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<D> PartialEq for StaticId<D> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<D> Eq for StaticId<D> {}
impl<D> fmt::Debug for StaticId<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StaticId")
    }
}

impl sealed::Sealed for LocalId<'_> {}
impl SlotIdentity for LocalId<'_> {}
impl<D> sealed::Sealed for StaticId<D> {}
impl<D> SlotIdentity for StaticId<D> {}
