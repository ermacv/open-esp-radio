//! Application stack setup and the small API difference between network contracts.
#[cfg(feature = "owned-network")]
mod owned;
#[cfg(feature = "owned-network")]
pub use owned::*;
