//! Ownership parts of the bootstrap; acquisition remains in the PAC.

use oer_mmio::{Fixed, Placement};

use crate::generated::soc::{
    hp_alive_sys::{self, HpAliveSys},
    hp_sys_clkrst::{self, HpSysClkrst},
    lp_aon_clkrst::{self, LpAonClkrst},
    pmu::{self, Pmu},
};

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ClockRegisters<
    P: Placement = Fixed<{ hp_sys_clkrst::BASE }>,
    Q: Placement = Fixed<{ lp_aon_clkrst::BASE }>,
    R: Placement = Fixed<{ pmu::BASE }>,
    S: Placement = Fixed<{ hp_alive_sys::BASE }>,
> {
    pub hp_sys_clkrst: HpSysClkrst<P>,
    pub lp_aon_clkrst: LpAonClkrst<Q>,
    pub pmu: Pmu<R>,
    pub hp_alive_sys: HpAliveSys<S>,
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement> ClockRegisters<P, Q, R, S> {
    pub(super) const fn new(
        hp_sys_clkrst: HpSysClkrst<P>,
        lp_aon_clkrst: LpAonClkrst<Q>,
        pmu: Pmu<R>,
        hp_alive_sys: HpAliveSys<S>,
    ) -> Self {
        Self {
            hp_sys_clkrst,
            lp_aon_clkrst,
            pmu,
            hp_alive_sys,
        }
    }
}
