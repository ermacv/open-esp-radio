//! Ownership parts of the bootstrap; acquisition remains in the PAC.

use oer_mmio::{Fixed, Placement};

use crate::generated::soc::{
    spi0::{self, Spi0},
    spi1::{self, Spi1},
};

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct FlashRegisters<
    P: Placement = Fixed<{ spi0::BASE }>,
    Q: Placement = Fixed<{ spi1::BASE }>,
> {
    pub spi0: Spi0<P>,
    pub spi1: Spi1<Q>,
}

impl<P: Placement, Q: Placement> FlashRegisters<P, Q> {
    pub(super) const fn new(spi0: Spi0<P>, spi1: Spi1<Q>) -> Self {
        Self { spi0, spi1 }
    }
}
