//! Closed identities of caller-owned port storage.

use core::{fmt, marker::PhantomData};

mod sealed {
    pub trait Sealed {}
}

/// The identity at the root of an installation's pool paths.
///
/// Only a generative [`LocalId`] implements this trait. An identity is a type marker; it is not construction or reuse
/// authority for the storage bearing that identity.
pub trait SlotIdentity: sealed::Sealed + Copy + Eq + fmt::Debug {}

/// A generative identity belonging to an outer local execution scope.
///
/// The scope is invariant: a shorter borrow cannot substitute another
/// scope's identity. The marker carries no global latch or mutable state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalId<'scope>(PhantomData<fn(&'scope ()) -> &'scope ()>);

impl sealed::Sealed for LocalId<'_> {}
impl SlotIdentity for LocalId<'_> {}
