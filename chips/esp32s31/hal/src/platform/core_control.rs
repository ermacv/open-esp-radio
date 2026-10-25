//! Core-control operations over the generated ownership parts.

use super::HpCore;
use oer_esp32s31_pac::{
    hp_sys_clkrst_core::{self, HpSysClkrstCore},
    hp_sys_clkrst_uart0::{self, HpSysClkrstUart0},
    hp_sys_core_stall_status::{self, HpSysCoreStallStatus},
    lp_aon_clkrst_core_reset::{self, LpAonClkrstCoreReset},
    pmu_cpu_stall::{self, PmuCpuStall},
};
use oer_mmio::{Bits, Fixed, Placement};

/// The stall request code that stalls a core.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`
/// (`cpu_utility_ll_stall_cpu`).
const STALL: u8 = 0x86;

/// The stall request code that lets a core run.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`
/// (`cpu_utility_ll_unstall_cpu`).
const RUN: u8 = 0xff;

/// SOURCE: ESP-IDF 4d59230d `uart_ll.h` (`uart_ll_set_sclk`): XTAL undivided.
const UART0_XTAL: Bits<2> = Bits::constant::<0>();
/// SOURCE: ESP-IDF 4d59230d `hp_sys_clkrst_reg.h:1408-1446`: zero integer and fractional codes.
const UART0_UNDIVIDED: u8 = 0;

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CoreControlRegisters<
    P: Placement = Fixed<{ pmu_cpu_stall::BASE }>,
    Q: Placement = Fixed<{ hp_sys_core_stall_status::BASE }>,
    R: Placement = Fixed<{ hp_sys_clkrst_core::BASE }>,
    S: Placement = Fixed<{ lp_aon_clkrst_core_reset::BASE }>,
> {
    stall: PmuCpuStall<P>,
    status: HpSysCoreStallStatus<Q>,
    clocks: HpSysClkrstCore<R>,
    reset: LpAonClkrstCoreReset<S>,
}

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Uart0ClockRegisters<P: Placement = Fixed<{ hp_sys_clkrst_uart0::BASE }>>(
    HpSysClkrstUart0<P>,
);
impl<P: Placement, Q: Placement, R: Placement, S: Placement> CoreControlRegisters<P, Q, R, S> {
    /// Request that `core` stall, keeping the other core's request.
    pub fn request_stall(&mut self, core: HpCore) {
        self.set_code(core, STALL);
    }

    /// Request that `core` run, keeping the other core's request.
    pub fn request_run(&mut self, core: HpCore) {
        self.set_code(core, RUN);
    }

    fn set_code(&mut self, core: HpCore, code: u8) {
        self.stall.modify_cpu_stall_sw(|request| {
            match core {
                HpCore::Core0 => request.set_hpcore0_sw_stall_code(code),
                HpCore::Core1 => request.set_hpcore1_sw_stall_code(code),
            };
        });
    }

    /// Whether `core`'s stall request asks it to stall: the request, not the
    /// hardware's confirmation ([`is_stalled`](Self::is_stalled)).
    #[cfg(test)]
    pub fn stall_requested(&self, core: HpCore) -> bool {
        let request = self.stall.read_cpu_stall_sw();
        let code = match core {
            HpCore::Core0 => request.hpcore0_sw_stall_code(),
            HpCore::Core1 => request.hpcore1_sw_stall_code(),
        };
        code == STALL
    }

    /// Whether the hardware reports `core` stalled.
    pub fn is_stalled(&self, core: HpCore) -> bool {
        let status = self.status.read_cpu_corestalled_st();
        match core {
            HpCore::Core0 => status.core0_corestalled_st(),
            HpCore::Core1 => status.core1_corestalled_st(),
        }
    }

    /// Whether HP core 1 is held: its CPU clock gated and its global reset
    /// asserted, as ESP-IDF's single-core start leaves it.
    pub fn core1_held(&self) -> bool {
        let control = self.clocks.read_hpcore1_ctrl0();
        control.core1_global_rst_en() && !control.core1_cpu_clk_en()
    }

    /// Hold HP core 1: gate its CPU and CLIC clocks and assert its global
    /// reset, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/esp_system/port/cpu_start.c`
    /// (the single-core start) and
    /// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`.
    pub fn hold_core1(&mut self) {
        self.clocks.modify_hpcore1_ctrl0(|control| {
            control
                .set_core1_cpu_clk_en(false)
                .set_core1_clic_clk_en(false)
                .set_core1_global_rst_en(true);
        });
    }

    /// Release HP core 1: enable its CPU and CLIC clocks and deassert its
    /// global reset, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/esp_system/port/cpu_start.c`
    /// (`start_other_core`).
    #[cfg(any(test, feature = "cpu1-runs"))]
    pub fn release_core1(&mut self) {
        self.clocks.modify_hpcore1_ctrl0(|control| {
            control
                .set_core1_cpu_clk_en(true)
                .set_core1_clic_clk_en(true)
                .set_core1_global_rst_en(false);
        });
    }

    /// Reset `core`'s CPU once: a pulse, not a hold.
    pub fn reset_core(&mut self, core: HpCore) {
        match core {
            HpCore::Core0 => self.reset.trigger_hpcore0_reset_ctrl_hpcore0_sw_reset(),
            HpCore::Core1 => self.reset.trigger_hpcore1_reset_ctrl_hpcore1_sw_reset(),
        }
    }
}
impl<P: Placement> Uart0ClockRegisters<P> {
    /// Return UART0's function clock to its power-on state, XTAL undivided
    /// with its gate open, keeping the bus clock and reset bits: the boot
    /// ROM prints through UART0 after a software reset, and the source the
    /// firmware chose may not run any more.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/soc/esp32s31/register/soc/hp_sys_clkrst_reg.h:1408-1446`
    /// (UART0 source, gate and divider fields) and
    /// `components/esp_hal_uart/esp32s31/include/hal/uart_ll.h`
    /// (`uart_ll_set_sclk`: XTAL selector zero).
    pub fn restore_uart0_clock(&mut self) {
        self.0.modify_uart0_ctrl0(|clock| {
            clock
                .set_clk_src_sel(UART0_XTAL)
                .set_sclk_div_num(UART0_UNDIVIDED)
                .set_sclk_div_numerator(UART0_UNDIVIDED)
                .set_sclk_div_denominator(UART0_UNDIVIDED)
                .set_clk_en(true);
        });
    }
}
#[cfg(any(test, feature = "test-support"))]
impl<'a>
    CoreControlRegisters<
        oer_mmio::Host<'a>,
        oer_mmio::Host<'a>,
        oer_mmio::Host<'a>,
        oer_mmio::Host<'a>,
    >
{
    /// Each block over its own host memory, which holds the words from the
    /// block's base to its last one.
    pub fn host(
        stall: &'a mut [u32],
        status: &'a mut [u32],
        clocks: &'a mut [u32],
        reset: &'a mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            stall: PmuCpuStall::host(stall)?,
            status: HpSysCoreStallStatus::host(status)?,
            clocks: HpSysClkrstCore::host(clocks)?,
            reset: LpAonClkrstCoreReset::host(reset)?,
        })
    }
}

/// The bytes of host memory each block needs, in the order
/// [`CoreControlRegisters::host`] takes them.
#[cfg(any(test, feature = "test-support"))]
pub const HOST_SIZES: [usize; 4] = [
    pmu_cpu_stall::SIZE,
    hp_sys_core_stall_status::SIZE,
    hp_sys_clkrst_core::SIZE,
    lp_aon_clkrst_core_reset::SIZE,
];

#[cfg(any(test, feature = "test-support"))]
impl<'a> Uart0ClockRegisters<oer_mmio::Host<'a>> {
    /// The block over host memory, which holds the words from the block's
    /// base to its last one.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        HpSysClkrstUart0::host(words).map(Self)
    }
}

/// The bytes of host memory UART0's clock block needs.
#[cfg(any(test, feature = "test-support"))]
pub const UART0_HOST_SIZE: usize = hp_sys_clkrst_uart0::SIZE;

impl<P: Placement, Q: Placement, R: Placement, S: Placement> CoreControlRegisters<P, Q, R, S> {
    pub(crate) fn new(parts: oer_esp32s31_pac::platform::CoreControlRegisters<P, Q, R, S>) -> Self {
        Self {
            stall: parts.stall,
            status: parts.status,
            clocks: parts.clocks,
            reset: parts.reset,
        }
    }
}
impl<P: Placement> Uart0ClockRegisters<P> {
    pub(crate) fn new(parts: oer_esp32s31_pac::platform::Uart0ClockRegisters<P>) -> Self {
        Self(parts.0)
    }
}
#[cfg(test)]
mod tests;
