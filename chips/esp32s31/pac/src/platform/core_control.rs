//! Core control: each HP core's stall request and stall status, HP core 1's
//! clocks and global reset and each core's software CPU reset
//! ([`CoreControlRegisters`]), and apart from them UART0's function clock,
//! which the boot ROM prints through after a software reset
//! ([`Uart0ClockRegisters`]).
//!
//! One handoff proof makes the owner of the four core-control blocks. The
//! HAL's cores module makes it only after its holder's compare-and-swap made
//! the caller the words' one writer: the cores' owner for one operation, or
//! the fatal path for good. UART0's clock has its own handoff and its own
//! holder, so a reset that cannot hold the core-control words still
//! restores it.

use oer_mmio::{Bits, Fixed, Placement};

use super::HpCore;
use crate::generated::{
    hp_sys_clkrst_core::{self, HpSysClkrstCore},
    hp_sys_clkrst_uart0::{self, HpSysClkrstUart0},
    hp_sys_core_stall_status::{self, HpSysCoreStallStatus},
    lp_aon_clkrst_core_reset::{self, LpAonClkrstCoreReset},
    peripheral_ownership::{
        CoreControlPeripherals, CoreControlPeripheralsClaims, Uart0ClockPeripherals,
        Uart0ClockPeripheralsClaims,
    },
    pmu_cpu_stall::{self, PmuCpuStall},
};

#[cfg(test)]
mod tests;

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

/// Proof that the caller holds the core-control words alone.
#[must_use = "the core-control owner needs the platform's transfer"]
pub struct CoreControlHandoff(());

impl CoreControlHandoff {
    /// Assert that the caller holds the core-control words alone.
    ///
    /// # Safety
    /// While the owner built from this proof lives, no other code accesses
    /// the words: no owner built from another proof lives at the same time,
    /// except one whose code never runs again (the world a fatal path
    /// stopped), and no esp-hal writer of them runs.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive core-control ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The core-control words.
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

impl CoreControlRegisters {
    /// Claim the four blocks from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: CoreControlHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks, from its handoff"
        )]
        // SAFETY: `CoreControlHandoff`'s constructor guarantees that no
        // other code accesses the words while this owner lives. The
        // generated `const` assertion beside the typed blocks checks that no
        // other typed block lists them.
        let claims = unsafe { oer_mmio::assume_owned::<CoreControlPeripheralsClaims>() };
        let blocks = CoreControlPeripherals::from_claims(claims);
        Self {
            stall: blocks.pmu_cpu_stall,
            status: blocks.hp_sys_core_stall_status,
            clocks: blocks.hp_sys_clkrst_core,
            reset: blocks.lp_aon_clkrst_core_reset,
        }
    }
}

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

/// Proof that the caller holds UART0's function clock word alone.
#[must_use = "the UART0 clock owner needs the platform's transfer"]
pub struct Uart0ClockHandoff(());

impl Uart0ClockHandoff {
    /// Assert that the caller holds UART0's function clock word alone.
    ///
    /// # Safety
    /// While the owner built from this proof lives, no other code accesses
    /// the word: no owner built from another proof lives at the same time,
    /// except one whose code never runs again, and no esp-hal writer of it
    /// runs.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive ownership of UART0's clock word"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// UART0's function clock word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Uart0ClockRegisters<P: Placement = Fixed<{ hp_sys_clkrst_uart0::BASE }>>(
    HpSysClkrstUart0<P>,
);

impl Uart0ClockRegisters {
    /// Claim the block from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: Uart0ClockHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks, from its handoff"
        )]
        // SAFETY: `Uart0ClockHandoff`'s constructor guarantees that no other
        // code accesses the word while this owner lives. The generated
        // `const` assertion beside the typed blocks checks that no other
        // typed block lists it.
        let claims = unsafe { oer_mmio::assume_owned::<Uart0ClockPeripheralsClaims>() };
        Self(Uart0ClockPeripherals::from_claims(claims).hp_sys_clkrst_uart0)
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
                .set_clk_src_sel(Bits::constant::<0>())
                .set_sclk_div_num(0)
                .set_sclk_div_numerator(0)
                .set_sclk_div_denominator(0)
                .set_clk_en(true);
        });
    }
}

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
