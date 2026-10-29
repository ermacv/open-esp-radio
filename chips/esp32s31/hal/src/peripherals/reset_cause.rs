//! Why each HP core last left reset, from the LP always-on reset-cause
//! words.

pub use oer_esp32s31_knowledge::reset_cause::ResetCause;
use oer_mmio::{Fixed, Placement};

use oer_esp32s31_pac::{
    instances, lp_aonclkrst::LpAonResetCause, ownership::ResetCausePeripherals,
};

use crate::platform::{HARTS, Hart};

#[cfg(test)]
mod tests;

/// The reset-cause words of both HP cores.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ResetCauseRegisters<P: Placement = Fixed<{ instances::lp_aon_clkrst::BASE }>>(
    LpAonResetCause<P>,
);

impl ResetCauseRegisters {
    /// Own the root's reset-cause partition.
    pub fn new(peripherals: ResetCausePeripherals) -> Self {
        Self(peripherals.lp_aon_reset_cause)
    }
}

/// The reset cause of each HP core, read once after boot: the words hold
/// until the next reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCauses {
    causes: [ResetCause; HARTS],
}

impl ResetCauses {
    /// Read both cores' causes; the owner is not needed again.
    pub fn read<P: Placement>(registers: ResetCauseRegisters<P>) -> Self {
        Self {
            causes: Hart::ALL.map(|hart| {
                let cause = registers
                    .0
                    .read_hpcore_reset_cause(hart.index())
                    .hpcore_reset_cause();
                ResetCause::from_code(cause.get().to_le_bytes()[0])
            }),
        }
    }

    /// Why `hart` last left reset.
    pub const fn of(self, hart: Hart) -> ResetCause {
        *hart.at(&self.causes)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> ResetCauseRegisters<oer_mmio::Host<'a>> {
    /// The reset-cause words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpAonResetCause::host(words).map(Self)
    }
}
