//! Nominal component identity inside an installation's pool layout.

use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// A path issued only by the pool factory. Paths occupy no storage.
///
/// The installation lifetime distinguishes installations; this path
/// distinguishes components, including nested components of the same kind.
pub trait PoolPath: sealed::Sealed + Copy + core::fmt::Debug + Eq + 'static {}

/// The root authority consumed by one installation's Mint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {}

/// Component `INDEX` of a tuple built at `Parent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Child<Parent: PoolPath, const INDEX: u8>(PhantomData<fn() -> Parent>);

impl sealed::Sealed for Root {}
impl PoolPath for Root {}
impl<P: PoolPath, const I: u8> sealed::Sealed for Child<P, I> {}
impl<P: PoolPath, const I: u8> PoolPath for Child<P, I> {}
