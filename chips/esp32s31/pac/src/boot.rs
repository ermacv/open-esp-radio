//! The blocks the bootstrap brings the chip up with, claimed from the soc
//! partition, and their implementations of the `oer-espressif-boot`
//! register traits.
//!
//! The bootstrap is its own ELF and its own ownership epoch: it claims the
//! soc partition once, from [`BootHandoff`], and its owners end with it when
//! it jumps to stage two. Each owner implements the register traits with
//! the typed fields, one read-modify-write per call, so every other bit of a
//! word, reserved ones included, is kept. The traits' calls need an
//! `oer_espressif_boot::Access`, which only that crate's sequences make.
//!
//! What is implemented here is what the soc render already writes: the CPU
//! root selection and the branch dividers, the CPU PLL's dividers and its
//! calibration, and instruction-fetch suspension. The clock update, the
//! cache sync engine's commands, the CPU PLL's power and the MMU's content
//! words wait for the renderer's command kind and the model's MMU fields.
//!
//! ```compile_fail,E0061
//! # use oer_esp32s31_pac::boot::ClockRegisters;
//! # use oer_espressif_boot::clock::CpuClockRegisters;
//! fn raw_stage(mut clock: ClockRegisters) {
//!     clock.stage(oer_esp32s31_pac::boot::CpuRoot::Cpll, Default::default()); // only a boot sequence stages
//! }
//! ```

use oer_espressif_boot::{
    Access,
    cache::FetchSuspendRegisters,
    clock::{CpuClockRegisters, PllRegisters},
};
use oer_mmio::{Bits, Fixed, Placement};

use crate::generated::soc::{
    cache::{self, Cache},
    hp_sys_clkrst::{self, HpSysClkrst},
    lp_aon_clkrst::{self, LpAonClkrst},
    peripheral_ownership::{Claims, PeripheralPartitions, SocPeripherals, partition},
};

#[cfg(test)]
mod tests;

/// Proof that the bootstrap owns the chip's soc blocks.
#[must_use = "the boot owners need the consumed bootstrap ownership"]
pub struct BootHandoff(());

impl BootHandoff {
    /// Assert the bootstrap's unique, permanent ownership of the soc blocks.
    ///
    /// # Safety
    /// Call at most once, in the bootstrap ELF, before any other code
    /// writes a soc block's word: no esp-hal driver, ROM helper or other
    /// owner of those words runs in the bootstrap's epoch.
    #[allow(
        unsafe_code,
        reason = "the bootstrap proves exclusive soc-block ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The bootstrap's owners of the soc blocks it brings the chip up with.
///
/// The other soc blocks are claimed with them and dropped: the bootstrap
/// writes no other block's word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct BootBlocks {
    /// The CPU clock root, its branch dividers and the CPU PLL.
    pub clock: ClockRegisters,
    /// The L1 cache's fetch-suspension words.
    pub cache: CacheRegisters,
}

impl BootBlocks {
    /// Claim the soc partition from the bootstrap's handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: BootHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `BootHandoff`'s constructor guarantees exclusive,
        // permanent ownership of the soc blocks in the bootstrap's epoch,
        // and it is consumed here, so the blocks are claimed once. The
        // generated `const` assertion beside the soc blocks checks that no
        // curated typed block lists their words.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_partitions(partition(claims))
    }

    fn from_partitions(partitions: PeripheralPartitions) -> Self {
        let SocPeripherals {
            cache,
            hp_sys_clkrst,
            lp_aon_clkrst,
            ..
        } = partitions.soc;
        Self {
            clock: ClockRegisters {
                hp_sys_clkrst,
                lp_aon_clkrst,
            },
            cache: CacheRegisters(cache),
        }
    }
}

/// The CPU's root clocks (`SOC_CLK_SEL`).
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:613-632`
/// (`clk_ll_cpu_set_src`: 0 XTAL, 1 CPLL, 2 RC_FAST, 3 PLL_F240M).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuRoot {
    Xtal,
    Cpll,
    RcFast,
    PllF240m,
}

impl CpuRoot {
    const fn code(self) -> Bits<2> {
        match self {
            Self::Xtal => Bits::constant::<0>(),
            Self::Cpll => Bits::constant::<1>(),
            Self::RcFast => Bits::constant::<2>(),
            Self::PllF240m => Bits::constant::<3>(),
        }
    }

    const fn from_code(code: Bits<2>) -> Self {
        match code.get() {
            0 => Self::Xtal,
            1 => Self::Cpll,
            2 => Self::RcFast,
            _ => Self::PllF240m,
        }
    }
}

/// How the memory clock divides the CPU clock (`MEM_CLK_DIV_NUM`, one bit).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemDivider {
    /// The memory clock is the CPU clock.
    #[default]
    Same,
    /// The memory clock is half the CPU clock.
    Half,
}

/// The integer divider of every branch the CPU root feeds: each branch
/// divides its source by its code plus one, with the fractional parts zero.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:475-541`
/// (CPU, memory, system and APB dividers; numerators and denominators 0)
/// and `esp-metadata-generated/src/_generated_esp32s31.rs:3662-3748` (CPU
/// from the root, memory and system from the CPU, APB from the system).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BusDividers {
    pub cpu: u8,
    pub mem: MemDivider,
    pub sys: u8,
    pub apb: u8,
}

/// The CPU PLL's divider pair (`LP_AONCLKRST_CPLL_DIV`): 320 MHz is the
/// 40 MHz XTAL times the feedback divider over the reference divider.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:209-213`
/// (feedback 8, reference 1 for 320 MHz; reset 15 / 2); ESP-IDF 4d59230d
/// `clk_tree_ll.h:448-453` (the CPLL runs at 320 MHz alone).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpllDividers {
    pub feedback: u8,
    pub reference: Bits<4>,
}

/// The CPU clock's root selection and dividers (`HP_SYS_CLKRST`) and the
/// CPU PLL's dividers (`LP_AON_CLKRST`).
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ClockRegisters<
    P: Placement = Fixed<{ hp_sys_clkrst::BASE }>,
    Q: Placement = Fixed<{ lp_aon_clkrst::BASE }>,
> {
    hp_sys_clkrst: HpSysClkrst<P>,
    lp_aon_clkrst: LpAonClkrst<Q>,
}

impl<P: Placement, Q: Placement> CpuClockRegisters for ClockRegisters<P, Q> {
    type Root = CpuRoot;
    type Dividers = BusDividers;

    fn root(&self, _: Access<'_>) -> Option<CpuRoot> {
        Some(CpuRoot::from_code(
            self.hp_sys_clkrst.read_soc_clk_sel().reg_soc_clk_sel(),
        ))
    }

    fn dividers(&self, _: Access<'_>) -> BusDividers {
        let clocks = &self.hp_sys_clkrst;
        BusDividers {
            cpu: clocks.read_cpu_freq_ctrl0().reg_cpu_clk_div_num(),
            mem: if clocks.read_mem_freq_ctrl0().reg_mem_clk_div_num() {
                MemDivider::Half
            } else {
                MemDivider::Same
            },
            sys: clocks.read_sys_freq_ctrl0().reg_sys_clk_div_num(),
            apb: clocks.read_apb_freq_ctrl0().reg_apb_clk_div_num(),
        }
    }

    fn stage(&mut self, _: Access<'_>, root: CpuRoot, dividers: BusDividers) {
        let clocks = &mut self.hp_sys_clkrst;
        let zero = Bits::constant::<0>();
        clocks.modify_cpu_freq_ctrl0(|w| {
            w.set_reg_cpu_clk_div_num(dividers.cpu)
                .set_reg_cpu_clk_div_numerator(zero)
                .set_reg_cpu_clk_div_denominator(zero);
        });
        clocks.modify_mem_freq_ctrl0(|w| {
            w.set_reg_mem_clk_div_num(dividers.mem == MemDivider::Half);
        });
        clocks.modify_sys_freq_ctrl0(|w| {
            w.set_reg_sys_clk_div_num(dividers.sys)
                .set_reg_sys_clk_div_numerator(zero)
                .set_reg_sys_clk_div_denominator(zero);
        });
        clocks.modify_apb_freq_ctrl0(|w| {
            w.set_reg_apb_clk_div_num(dividers.apb)
                .set_reg_apb_clk_div_numerator(zero)
                .set_reg_apb_clk_div_denominator(zero);
        });
        clocks.modify_soc_clk_sel(|w| {
            w.set_reg_soc_clk_sel(root.code());
        });
    }
}

/// The CPU PLL's divider pair and calibration.
///
/// SOURCE: ESP-IDF 4d59230d `clk_tree_ll.h:458-479` (calibration started by
/// clearing `CPU_PLL_CAL_STOP`, ended when `CPU_PLL_CAL_END` reads 1,
/// stopped by setting `CPU_PLL_CAL_STOP`).
impl<P: Placement, Q: Placement> PllRegisters for ClockRegisters<P, Q> {
    type Target = CpllDividers;

    fn target(&self, _: Access<'_>) -> CpllDividers {
        let divider = self.lp_aon_clkrst.read_lp_aonclkrst_cpll_div();
        CpllDividers {
            feedback: divider.lp_aonclkrst_cpll_fb_div(),
            reference: divider.lp_aonclkrst_cpll_ref_div(),
        }
    }

    fn set_target(&mut self, _: Access<'_>, target: CpllDividers) {
        self.lp_aon_clkrst.modify_lp_aonclkrst_cpll_div(|w| {
            w.set_lp_aonclkrst_cpll_fb_div(target.feedback)
                .set_lp_aonclkrst_cpll_ref_div(target.reference);
        });
    }

    fn start_calibration(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_ana_pll_ctrl0(|w| {
            w.set_reg_cpu_pll_cal_stop(false);
        });
    }

    fn calibration_done(&self, _: Access<'_>) -> bool {
        self.hp_sys_clkrst
            .read_ana_pll_ctrl0()
            .reg_cpu_pll_cal_end()
    }

    fn stop_calibration(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_ana_pll_ctrl0(|w| {
            w.set_reg_cpu_pll_cal_stop(true);
        });
    }
}

/// The caches a sync command acts on (`SYNC_MAP`).
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/cache.rs:3-4` (data
/// cache 0x10, both instruction caches 0x03); ESP-IDF 4d59230d
/// `components/soc/esp32s31/register/soc/cache_reg.h:1630-1640`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheMap {
    Data,
    BothInstructionCaches,
}

/// The L1 cache's words for core 0's instruction-fetch suspension.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CacheRegisters<P: Placement = Fixed<{ cache::BASE }>>(Cache<P>);

/// Core 0's instruction-fetch suspension on the L1 cache: instruction bus
/// 0, instruction cache 0's autoload, preload and freeze, the sync and lock
/// engines' idle flags.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32s31/register/soc/cache_reg.h:16-22`
/// (`L1_ICACHE_SHUT_IBUS0`), `:805-829` (`L1_ICACHE0_FREEZE_EN`, `_MODE`,
/// `_DONE`), `:1496-1520` (`LOCK_DONE`), `:1574-1620` (`SYNC_DONE`),
/// `:1672-1684` (`L1_ICACHE0_PRELOAD_DONE`), `:2049-2061`
/// (`L1_ICACHE0_AUTOLOAD_ENA`, `_DONE`); the pinned rev0 ROM's
/// `Cache_Suspend_L1_CORE0_ICache` uses exactly these fields
/// (`oer_espressif_boot::cache`).
impl<P: Placement> FetchSuspendRegisters for CacheRegisters<P> {
    fn shut_fetch(&mut self, _: Access<'_>, shut: bool) {
        self.0.modify_l1_icache_ctrl(|w| {
            w.set_l1_icache_shut_ibus0(shut);
        });
    }

    fn autoload_enabled(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_ena()
    }

    fn set_autoload(&mut self, _: Access<'_>, enabled: bool) {
        self.0.modify_l1_icache0_autoload_ctrl(|w| {
            w.set_l1_icache0_autoload_ena(enabled);
        });
    }

    fn autoload_done(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_done()
    }

    fn preload_done(&self, _: Access<'_>) -> bool {
        self.0
            .read_l1_icache0_preload_ctrl()
            .l1_icache0_preload_done()
    }

    fn sync_done(&self, _: Access<'_>) -> bool {
        self.0.read_sync_ctrl().sync_done()
    }

    fn lock_done(&self, _: Access<'_>) -> bool {
        self.0.read_lock_ctrl().lock_done()
    }

    fn select_freeze_mode_zero(&mut self, _: Access<'_>) {
        self.0.modify_l1_cache_freeze_ctrl(|w| {
            w.set_l1_icache0_freeze_mode(false);
        });
    }

    fn set_freeze(&mut self, _: Access<'_>, frozen: bool) {
        self.0.modify_l1_cache_freeze_ctrl(|w| {
            w.set_l1_icache0_freeze_en(frozen);
        });
    }

    fn freeze_done(&self, _: Access<'_>) -> bool {
        self.0.read_l1_cache_freeze_ctrl().l1_icache0_freeze_done()
    }
}

#[cfg(test)]
impl<'a, 'b> ClockRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// The clock owner over host memory.
    pub(crate) fn host(
        hp_sys_clkrst: &'a mut [u32],
        lp_aon_clkrst: &'b mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            hp_sys_clkrst: HpSysClkrst::host(hp_sys_clkrst)?,
            lp_aon_clkrst: LpAonClkrst::host(lp_aon_clkrst)?,
        })
    }
}

#[cfg(test)]
impl<'a> CacheRegisters<oer_mmio::Host<'a>> {
    /// The cache owner over host memory.
    pub(crate) fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Cache::host(words).map(Self)
    }
}
