//! Ownership parts of the bootstrap; acquisition remains in the PAC.

use oer_mmio::{Fixed, Placement};

use crate::generated::soc::cache::{self, Cache};

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CacheRegisters<P: Placement = Fixed<{ cache::BASE }>>(pub Cache<P>);

impl<P: Placement> CacheRegisters<P> {
    pub(super) const fn new(cache: Cache<P>) -> Self {
        Self(cache)
    }
}
