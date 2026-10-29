//! Connection epochs of one installation. The backend mints one source
//! slot for its station; moving that slot into the source consumes it.
//! Dropping/recreating a station cannot mint another source in that install.

use core::marker::PhantomData;
use oer_ieee80211_port::ConnectionEpochKind;
use oer_radio_port::{ContextLoan, ContextProjection, PortContext};
use oer_radio_port::{PoolPath, Root, Slot};

/// Queue representation of an epoch and its operation data. The epoch is
/// private; public projections always restore the consumer's body loan.
pub struct EpochContext<Path: PoolPath, D> {
    sequence: u64,
    data: D,
    path: PhantomData<fn(Path) -> Path>,
}

impl<'p, Path: PoolPath> LinkEpoch<'p, Path> {
    /// Read-only publication identity for a neutral network port. This does
    /// not grant permission to construct a branded station epoch.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Enclose this epoch and its data for an opaque backend context.
    pub fn context<D>(self, data: D) -> PortContext<'p, Path::Identity, EpochContext<Path, D>> {
        PortContext::new(EpochContext {
            sequence: self.sequence,
            data,
            path: PhantomData,
        })
    }
}

impl<Path: PoolPath, D> ContextProjection<Path::Identity> for EpochContext<Path, D> {
    type View<'a, 'p>
        = (LinkEpoch<'p, Path>, &'a D)
    where
        Self: 'p,
        Path::Identity: 'p,
        'p: 'a;
    type Parts<'p>
        = (LinkEpoch<'p, Path>, D)
    where
        Self: 'p,
        Path::Identity: 'p;

    fn view<'a, 'p>(&'a self, _: ContextLoan<'p, Path::Identity>) -> Self::View<'a, 'p>
    where
        Self: 'p,
        Path::Identity: 'p,
        'p: 'a,
    {
        (
            LinkEpoch {
                sequence: self.sequence,
                brand: PhantomData,
            },
            &self.data,
        )
    }

    fn into_parts<'p>(self, _: ContextLoan<'p, Path::Identity>) -> Self::Parts<'p>
    where
        Self: 'p,
        Path::Identity: 'p,
    {
        (
            LinkEpoch {
                sequence: self.sequence,
                brand: PhantomData,
            },
            self.data,
        )
    }
}

/// A connection identity, invariant in the installation's brand. Only the
/// station's source issues one; neither callers nor Data reconstruct it.
///
/// ```compile_fail
/// use oer_ieee80211_sta::epoch::LinkEpoch;
/// fn cross_install<'old, 'new>(old: LinkEpoch<'old>) -> LinkEpoch<'new> { old }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkEpoch<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    sequence: u64,
    brand: PhantomData<fn(&'id (), Path) -> &'id ()>,
}

/// The station's sole, non-cloneable source. A new source consumes the
/// install's unique slot. It has no reset and never gives that slot back;
/// reset requires ending the install, which also ends its branded contexts.
pub struct LinkEpochSource<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    brand: PhantomData<fn(&'id (), Path) -> &'id ()>,
    last: u64,
}

impl<'id, Path: PoolPath> LinkEpochSource<'id, Path> {
    /// Consume the one station source slot the backend minted at install.
    pub fn new(_slot: Slot<'id, ConnectionEpochKind, Path>) -> Self {
        Self {
            brand: PhantomData,
            last: 0,
        }
    }

    /// Issue the next connection identity, including across `u64::MAX`.
    ///
    /// Only the current connection and operations still owned by the backend
    /// need distinct identities. Tickets bound the number of those operations,
    /// and teardown returns all of them before the installation ends. An old
    /// identity can coincide with a new one only if an operation survives a
    /// full 2^64 reconnections in this installation. The lifecycle excludes
    /// that survival; the ticket bound alone bounds count, not lifetime.
    /// This is the owner's modular-epoch contract, not a Rust proof of age.
    pub(crate) fn begin(&mut self) -> LinkEpoch<'id, Path> {
        self.last = self.last.wrapping_add(1);
        LinkEpoch {
            sequence: self.last,
            brand: PhantomData,
        }
    }
}

#[cfg(test)]
impl<'id, Path: PoolPath> LinkEpoch<'id, Path> {
    pub(crate) const fn for_test(sequence: u64) -> Self {
        Self {
            sequence,
            brand: PhantomData,
        }
    }
}

#[cfg(test)]
impl<'id> LinkEpochSource<'id, Root<oer_radio_port::LocalId<'id>>> {
    pub(crate) const fn for_test(last: u64) -> Self {
        Self {
            brand: PhantomData,
            last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_epoch_context_preserves_a_non_send_owner_and_the_consumers_epoch_loan() {
        use oer_radio_port::{ContextChoice, Root, StaticId};
        use std::rc::Rc;
        enum Declaration {}
        type Path = Root<StaticId<Declaration>>;
        let owner = Rc::new(42);
        let address = Rc::as_ptr(&owner);
        let epoch = LinkEpoch::<'_, Path>::for_test(123);
        let context = epoch.context(owner).first::<()>();
        let ContextChoice::First(context) = context.into_choice() else {
            panic!("the context family is preserved");
        };
        let (borrowed, data) = context.view();
        assert_eq!(borrowed.sequence, 123);
        assert_eq!(Rc::strong_count(data), 1);
        assert_eq!(Rc::as_ptr(data), address);
        let (returned, owner) = context.into_parts();
        assert_eq!(returned.sequence, epoch.sequence);
        assert_eq!(Rc::as_ptr(&owner), address);
        assert_eq!(Rc::strong_count(&owner), 1);
    }

    #[test]
    fn adjacent_live_epochs_remain_distinct_across_wrap() {
        let mut source = LinkEpochSource::for_test(u64::MAX - 1);
        let last = source.begin();
        let wrapped = source.begin();
        let next = source.begin();
        assert_eq!(last.sequence, u64::MAX);
        assert_eq!(wrapped.sequence, 0);
        assert_eq!(next.sequence, 1);
        assert_ne!(last, wrapped);
        assert_ne!(wrapped, next);
    }
}
