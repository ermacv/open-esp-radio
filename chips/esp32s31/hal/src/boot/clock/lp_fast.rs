//! The LP-fast selector atom; source and settle belong to the chip profile.
pub use oer_esp32s31_pac::lp_aonclkrst::root_clk_conf::FastClkSel as LpFastSource;
use oer_esp32s31_pac::{instances, lp_aonclkrst::LpAonRadioClock};
use oer_espressif_boot::{Access, drivers::clock::LpFastRegisters};
use oer_mmio::{Fixed, Placement};

/// The existing LP root word, owned by the bootstrap before LP consumers.
/// Runtime PHY receives this part separately through its runtime root.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct LpFastClock<P: Placement = Fixed<{ instances::lp_aon_clkrst::BASE }>>(
    LpAonRadioClock<P>,
);

impl LpFastClock {
    pub(crate) fn new(part: LpAonRadioClock) -> Self {
        Self(part)
    }
}

impl<P: Placement> LpFastRegisters for LpFastClock<P> {
    type Source = LpFastSource;
    fn write_source(&mut self, _: Access<'_>, source: Self::Source) {
        self.0.modify_root_clk_conf(|word| {
            word.set_fast_clk_sel(source);
        });
    }
    fn source(&self, _: Access<'_>) -> u32 {
        self.0.read_root_clk_conf().fast_clk_sel().get()
    }
    fn source_code(source: Self::Source) -> u32 {
        source.bits()
    }
}
