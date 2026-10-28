//! The bootstrap's LP-fast root, selected explicitly from profile facts.

use oer_esp32s31_pac::{
    instances,
    lp_aonclkrst::{LpAonRadioClock, root_clk_conf},
};
use oer_espressif_boot::{
    BootFailure, ClockFailure,
    wait::{BootDeadline, BootWait},
};
use oer_mmio::{Bits, Fixed, Placement};
use oer_time::{Clock, Duration};

include!(concat!(env!("OUT_DIR"), "/lp-fast-source.rs"));

/// SOURCE: ESP-IDF 4d59230ddff16327812782151ef0afef202dc6d7
/// `components/esp_hw_support/port/esp32s31/rtc_clk.c:122-126`
/// (`rtc_clk_fast_src_set`), `include/soc/rtc.h:56`
/// (`SOC_DELAY_RTC_FAST_CLK_SWITCH`, 3 µs after the selector write).
const SWITCH_SETTLE: BootWait = const {
    match BootWait::new("lp-fast-switch", Duration::from_micros(3)) {
        Ok(wait) => wait,
        Err(_) => panic!("LP-fast switch settle has a nonzero bound"),
    }
};

const _: () = {
    if !crate::boot::BOOT_BUDGET.admits(SWITCH_SETTLE) {
        panic!("LP-fast switch settle must fit the boot budget");
    }
};

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

impl<P: Placement> LpFastClock<P> {
    /// Write the facts-selected source, preserve every other field, wait
    /// for the switch and require the selector to read back identically.
    /// RC_FAST is not a supported profile policy.
    ///
    /// # Errors
    ///
    /// The common boot deadline cannot admit the settle, or the selector
    /// disagrees with the policy (including unnamed hardware codes).
    pub fn select<C: Clock + ?Sized>(
        &mut self,
        deadline: &BootDeadline<'_, C>,
    ) -> Result<(), BootFailure> {
        select(self, deadline)
    }
}

/// The register operations used by the selection sequence.
trait Registers {
    fn write_source(&mut self, source: root_clk_conf::FastClkSel);
    fn source(&self) -> Bits<2>;
}

impl<P: Placement> Registers for LpFastClock<P> {
    fn write_source(&mut self, source: root_clk_conf::FastClkSel) {
        self.0.modify_root_clk_conf(|word| {
            word.set_fast_clk_sel(source);
        });
    }

    fn source(&self) -> Bits<2> {
        self.0.read_root_clk_conf().fast_clk_sel()
    }
}

fn select<C: Clock + ?Sized>(
    registers: &mut impl Registers,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure> {
    registers.write_source(LP_FAST_SOURCE);
    deadline.hold(SWITCH_SETTLE)?;
    let observed = registers.source().get();
    let expected = LP_FAST_SOURCE.bits();
    if observed != expected {
        return Err(ClockFailure::LpFastReadback { expected, observed }.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
