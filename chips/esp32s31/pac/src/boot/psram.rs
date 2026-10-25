//! Ownership parts of the bootstrap; acquisition remains in the PAC.

use oer_mmio::{Fixed, Placement};

use crate::generated::soc::{
    iomux_mspi_pin::{self, IomuxMspiPin},
    memspi2::{self, Memspi2},
    memspi3::{self, Memspi3},
};

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct PsramRegisters<
    P: Placement = Fixed<{ memspi2::BASE }>,
    Q: Placement = Fixed<{ memspi3::BASE }>,
    R: Placement = Fixed<{ iomux_mspi_pin::BASE }>,
> {
    pub memspi2: Memspi2<P>,
    pub memspi3: Memspi3<Q>,
    pub pins: IomuxMspiPin<R>,
}

impl<P: Placement, Q: Placement, R: Placement> PsramRegisters<P, Q, R> {
    pub(super) const fn new(
        memspi2: Memspi2<P>,
        memspi3: Memspi3<Q>,
        pins: IomuxMspiPin<R>,
    ) -> Self {
        Self {
            memspi2,
            memspi3,
            pins,
        }
    }
}
