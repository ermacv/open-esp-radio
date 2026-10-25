//! The clock owner: the CPU root and dividers, the CPU PLL and its power,
//! the clock update, the flash clock, the memory PLL and the PSRAM's supply.

use oer_espressif_boot::{
    Access, Busy,
    clock::{ClockUpdate, CpuClockRegisters, PllPower, PllRegisters},
    flash::FlashClockRegisters,
    psram::MemoryClockRegisters,
};
use oer_mmio::{Bits, Fixed, Placement};

use oer_esp32s31_pac::soc::{
    hp_alive_sys::{self, HpAliveSys},
    hp_sys_clkrst::{self, HpSysClkrst, flash_ctrl0},
    lp_aon_clkrst::{self, LpAonClkrst},
    pmu::{self, Pmu},
};

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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:664-762`
/// (CPU from the root, memory/system from CPU, APB from system; divider
/// fields hold the integer divisor minus one).
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:410-453`
/// (feedback 8, reference 1 for 320 MHz) and
/// `components/soc/esp32s31/register/soc/lp_clkrst_reg.h` (`CPLL_DIV`: reset 15/2).
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
    R: Placement = Fixed<{ pmu::BASE }>,
    S: Placement = Fixed<{ hp_alive_sys::BASE }>,
> {
    hp_sys_clkrst: HpSysClkrst<P>,
    lp_aon_clkrst: LpAonClkrst<Q>,
    pmu: Pmu<R>,
    hp_alive_sys: HpAliveSys<S>,
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement> ClockRegisters<P, Q, R, S> {
    /// Own the bootstrap's generated clock parts.
    pub fn new(parts: oer_esp32s31_pac::boot::ClockRegisters<P, Q, R, S>) -> Self {
        Self {
            hp_sys_clkrst: parts.hp_sys_clkrst,
            lp_aon_clkrst: parts.lp_aon_clkrst,
            pmu: parts.pmu,
            hp_alive_sys: parts.hp_alive_sys,
        }
    }
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement> CpuClockRegisters
    for ClockRegisters<P, Q, R, S>
{
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
impl<P: Placement, Q: Placement, R: Placement, S: Placement> PllRegisters
    for ClockRegisters<P, Q, R, S>
{
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

/// The SOC clock update (`ROOT_CLK_CTRL0.SOC_CLK_UPDATE`): it latches the
/// staged root and every divider at once, and clears when done.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:602-606`.
impl<P: Placement, Q: Placement, R: Placement, S: Placement> ClockUpdate
    for ClockRegisters<P, Q, R, S>
{
    fn start_update(&mut self, _: Access<'_>) -> Result<(), Busy> {
        self.hp_sys_clkrst
            .start_root_clk_ctrl0_reg_soc_clk_update()
            .map_err(|_| Busy)
    }

    fn update_pending(&self, _: Access<'_>) -> bool {
        self.hp_sys_clkrst.root_clk_ctrl0_reg_soc_clk_update_busy()
    }
}

/// A PLL's lane in the PMU's immediate tie fields (`IMM_HP_CK_POWER_1`):
/// each 4-bit tie field holds one bit per PLL.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/soc/esp32s31/register/soc/pmu_reg.h:1964-2133` (the
/// per-PLL bits: CPLL the lowest, MPLL the highest of each field).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PllLane {
    Cpll,
    Mpll,
}

impl PllLane {
    const fn bit(self) -> Bits<4> {
        match self {
            Self::Cpll => Bits::constant::<0b0001>(),
            Self::Mpll => Bits::constant::<0b1000>(),
        }
    }
}

/// The CPU PLL powered: its power and analog-I2C ties, its clock gate tie,
/// then its 300-MHz clock gate.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:93-99`
/// (`clk_ll_cpll_enable`; its active-state power belongs to the radio PMU
/// block's owner).
impl<P: Placement, Q: Placement, R: Placement, S: Placement> PllPower
    for ClockRegisters<P, Q, R, S>
{
    fn power_up(&mut self, _: Access<'_>) {
        let lane = PllLane::Cpll.bit();
        self.pmu.trigger_imm_hp_ck_power_1_tie_high_xpd_pll(lane);
        self.pmu
            .trigger_imm_hp_ck_power_1_tie_high_xpd_pll_i2c(lane);
        self.pmu
            .trigger_imm_hp_ck_power_1_tie_high_global_pll_icg(lane);
        self.hp_alive_sys.modify_hp_clk_ctrl(|w| {
            w.set_hp_cpll_300m_clk_en(true);
        });
    }
}

/// The flash clock as the bootloader left it, restored after a failed sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashClockWord(flash_ctrl0::R);

/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:73-90`:
/// SPLL / 4, the 120 MHz flash tuning clock.
const FLASH_TUNING_SOURCE: Bits<2> = Bits::constant::<1>();
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:73-90`: divisor minus one.
const FLASH_TUNING_DIVIDER: u8 = 3;

/// The flash clock (`FLASH_CTRL0`): the sweep's rate is the 480-MHz SPLL
/// divided by four, 120 MHz, its gates open.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:73-90`
/// (`set_flash_clock_120mhz`: source 1, divider field 3).
impl<P: Placement, Q: Placement, R: Placement, S: Placement> FlashClockRegisters
    for ClockRegisters<P, Q, R, S>
{
    type Saved = FlashClockWord;

    fn save_clock(&self, _: Access<'_>) -> FlashClockWord {
        FlashClockWord(self.hp_sys_clkrst.read_flash_ctrl0())
    }

    fn select_tuning_clock(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_flash_ctrl0(|w| {
            w.set_reg_flash_sys_clk_en(true)
                .set_reg_flash_pll_clk_en(true)
                .set_reg_flash_core_clk_en(true)
                .set_reg_flash_clk_src_sel(FLASH_TUNING_SOURCE)
                .set_reg_flash_core_clk_div_num(FLASH_TUNING_DIVIDER);
        });
    }

    fn restore_clock(&mut self, _: Access<'_>, FlashClockWord(saved): FlashClockWord) {
        self.hp_sys_clkrst.modify_flash_ctrl0(|w| {
            w.set_reg_flash_sys_clk_en(saved.reg_flash_sys_clk_en())
                .set_reg_flash_pll_clk_en(saved.reg_flash_pll_clk_en())
                .set_reg_flash_core_clk_en(saved.reg_flash_core_clk_en())
                .set_reg_flash_clk_src_sel(saved.reg_flash_clk_src_sel())
                .set_reg_flash_core_clk_div_num(saved.reg_flash_core_clk_div_num());
        });
    }
}

/// A level of the PSRAM PHY's external LDO: its reference and multiplier
/// codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PsramSupplyLevel {
    pub dref: Bits<4>,
    pub mul: Bits<3>,
}

/// SOURCE: ESP-IDF 4d59230d `psram_ctrlr_ll.h:414-433`: source 1 is MPLL.
const PSRAM_MEMORY_PLL_SOURCE: Bits<2> = Bits::constant::<1>();
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:283-351`.
const REFERENCE_25M_DIVIDER: u8 = 19;
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:283-351`.
const REFERENCE_50M_DIVIDER: u8 = 9;

/// The memory PLL (MPLL), the PSRAM PHY's supply and the PSRAM
/// controller's clocks and reset (`PSRAM_CTRL0`).
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_mspi/esp32s31/include/hal/psram_ctrlr_ll.h:414-433`
/// (the source code 1, MPLL) and
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:200-208`
/// (`clk_ll_mpll_enable`: the gate tie, the power ties, the 500-MHz gate);
/// esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:283-351` (the
/// reference dividers 19 and 9, the 500-MHz reference from MPLL, the LDO's
/// fields) and `esp-hal/src/psram/esp32s31.rs:173-187`,
/// `esp-hal/src/soc/esp32s31/clocks.rs:746-768` (the controller's gates and
/// reset).
impl<P: Placement, Q: Placement, R: Placement, S: Placement> MemoryClockRegisters
    for ClockRegisters<P, Q, R, S>
{
    type SupplyLevel = PsramSupplyLevel;

    fn open_controller_clock(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_psram_ctrl0(|w| {
            w.set_reg_psram_sys_clk_en(true);
        });
    }

    fn reset_controller(&mut self, _: Access<'_>, asserted: bool) {
        self.hp_sys_clkrst.modify_psram_ctrl0(|w| {
            w.set_reg_psram_axi_rst_en(asserted)
                .set_reg_psram_apb_rst_en(asserted);
        });
    }

    fn select_memory_pll(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_psram_ctrl0(|w| {
            w.set_reg_psram_clk_src_sel(PSRAM_MEMORY_PLL_SOURCE);
        });
    }

    fn limit_supply_current(&mut self, _: Access<'_>, limited: bool) {
        self.pmu.modify_ext_ldo_ctrl(|w| {
            w.set_ext_cur_lim(limited);
        });
    }

    fn set_supply_level(&mut self, _: Access<'_>, level: PsramSupplyLevel) {
        self.pmu.modify_ext_ldo_ctrl(|w| {
            w.set_ext_ldo_mul(level.mul)
                .set_ext_ldo_dref(level.dref)
                .set_ext_ldo_tie_high(false);
        });
    }

    fn enable_supply_detector(&mut self, _: Access<'_>) {
        self.pmu.modify_ext_ldo_ctrl(|w| {
            w.set_ext_ldo_en_vdet(true);
        });
    }

    fn power_supply(&mut self, _: Access<'_>) {
        self.pmu.modify_psram_cfg(|w| {
            w.set_psram_xpd(true);
        });
    }

    fn set_memory_pll_references(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_ref_25m_ctrl0(|w| {
            w.set_reg_ref_25m_clk_div_num(REFERENCE_25M_DIVIDER);
        });
        self.hp_sys_clkrst.modify_ref_50m_ctrl0(|w| {
            w.set_reg_ref_50m_clk_div_num(REFERENCE_50M_DIVIDER);
        });
        self.hp_sys_clkrst.modify_ref_500m_ctrl0(|w| {
            w.set_reg_ref_500m_sel(true);
        });
    }

    fn power_memory_pll(&mut self, _: Access<'_>) {
        let lane = PllLane::Mpll.bit();
        self.pmu
            .trigger_imm_hp_ck_power_1_tie_high_global_pll_icg(lane);
        self.pmu.trigger_imm_hp_ck_power_1_tie_high_xpd_pll(lane);
        self.pmu
            .trigger_imm_hp_ck_power_1_tie_high_xpd_pll_i2c(lane);
        self.hp_alive_sys.modify_hp_clk_ctrl(|w| {
            w.set_hp_mpll_500m_clk_en(true);
        });
    }

    fn open_function_clock(&mut self, _: Access<'_>) {
        self.hp_sys_clkrst.modify_psram_ctrl0(|w| {
            w.set_reg_psram_pll_clk_en(true)
                .set_reg_psram_core_clk_en(true);
        });
    }
}

#[cfg(test)]
impl<'a, 'b, 'c, 'd>
    ClockRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>, oer_mmio::Host<'c>, oer_mmio::Host<'d>>
{
    /// The clock owner over host memory.
    pub(crate) fn host(
        hp_sys_clkrst: &'a mut [u32],
        lp_aon_clkrst: &'b mut [u32],
        pmu: &'c mut [u32],
        hp_alive_sys: &'d mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            hp_sys_clkrst: HpSysClkrst::host(hp_sys_clkrst)?,
            lp_aon_clkrst: LpAonClkrst::host(lp_aon_clkrst)?,
            pmu: Pmu::host(pmu)?,
            hp_alive_sys: HpAliveSys::host(hp_alive_sys)?,
        })
    }
}
