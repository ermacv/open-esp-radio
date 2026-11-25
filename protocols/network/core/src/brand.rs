//! The brands of networks, and the right to claim one.

use core::{fmt, marker::PhantomData};

mod sealed {
    pub trait Sealed {}
}

/// A network's identity, which its blocks, groups, loans and handles carry.
///
/// Only a generative [`LocalNetwork`] implements it. A brand is a type
/// marker: naming it is no right to its pool.
pub trait Brand: sealed::Sealed + Copy + Eq + fmt::Debug {}

/// The brand of one [`with_local_network`] scope, invariant in it: a shorter
/// borrow stands for no other scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalNetwork<'n>(PhantomData<fn(&'n ()) -> &'n ()>);

impl sealed::Sealed for LocalNetwork<'_> {}
impl Brand for LocalNetwork<'_> {}

/// The right to brand `B`'s one pool and the right to its core, which
/// [`claim`](crate::claim) consumes.
///
/// It is made once for its brand and nothing copies it, so a second claim
/// does not compile:
///
/// ```compile_fail,E0382
/// # use oer_protocols_network_core::{claim, with_local_network};
/// # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1]) {
/// with_local_network(|right| {
///     let first = claim(right, a);
///     let second = claim(right, b);
/// });
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use oer_protocols_network_core::with_local_network;
/// with_local_network(|right| {
///     let second = right.clone();
/// });
/// ```
///
/// Nothing but the scope makes one:
///
/// ```compile_fail,E0451
/// use oer_protocols_network_core::{LocalNetwork, NetworkClaim};
/// fn made<'n>() -> NetworkClaim<LocalNetwork<'n>> {
///     NetworkClaim { brand: core::marker::PhantomData }
/// }
/// ```
#[derive(Debug)]
#[must_use = "the right is the brand's one claim"]
pub struct NetworkClaim<B: Brand> {
    pub(crate) brand: PhantomData<fn(B) -> B>,
}

/// Run `scope` with the right to a brand of its own. The brand lives as long
/// as the scope, and nothing that carries it leaves:
///
/// ```compile_fail
/// # use oer_protocols_network_core::with_local_network;
/// let right = with_local_network(|right| right);
/// ```
pub fn with_local_network<R>(scope: impl for<'n> FnOnce(NetworkClaim<LocalNetwork<'n>>) -> R) -> R {
    scope(NetworkClaim { brand: PhantomData })
}
