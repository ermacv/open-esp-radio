//! Core control over the root's parts: each HP core's stall request and
//! stall status, per-hart clocks and global resets, each core's software
//! CPU reset, secondary boot addresses, the system reset trigger and
//! UART0's function clock, which the boot ROM prints through
//! after a software reset.

use super::{Hart, SecondaryHart};
use oer_esp32s31_pac::{
    hp_sys_clkrst::{HpSysClkrstCore, HpSysClkrstUart0},
    hp_system::HpSysCoreStallStatus,
    instances,
    lp_aonclkrst::LpAonClkrstCoreReset,
    lp_system_reg::LpSysCoreControl,
    ownership::{CoreControlPeripherals, Uart0ClockPeripherals},
    pmu::PmuCpuStall,
};
use oer_mmio::{Bits, Fixed, Placement};

#[cfg(test)]
mod tests;

/// The stall request code that stalls a core.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`
/// (`cpu_utility_ll_stall_cpu`).
const STALL: u8 = 0x86;

/// The stall request code that lets a core run.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`
/// (`cpu_utility_ll_unstall_cpu`).
const RUN: u8 = 0xff;

/// SOURCE: ESP-IDF 4d80c1b2 `uart_ll.h` (`uart_ll_set_sclk`): XTAL undivided.
const UART0_XTAL: Bits<2> = Bits::constant::<0>();
/// SOURCE: ESP-IDF 4d80c1b2 `hp_sys_clkrst_reg.h:1408-1446`: zero integer and fractional codes.
const UART0_UNDIVIDED: u8 = 0;

/// The core-control words.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CoreControlRegisters<
    P: Placement = Fixed<{ instances::pmu::BASE }>,
    Q: Placement = Fixed<{ instances::hp_sys::BASE }>,
    R: Placement = Fixed<{ instances::hp_sys_clkrst::BASE }>,
    S: Placement = Fixed<{ instances::lp_aon_clkrst::BASE }>,
    T: Placement = Fixed<{ instances::lp_sys::BASE }>,
> {
    stall: PmuCpuStall<P>,
    status: HpSysCoreStallStatus<Q>,
    clocks: HpSysClkrstCore<R>,
    reset: LpAonClkrstCoreReset<S>,
    system: LpSysCoreControl<T>,
}

/// UART0's function clock word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Uart0ClockRegisters<P: Placement = Fixed<{ instances::hp_sys_clkrst::BASE }>>(
    HpSysClkrstUart0<P>,
);

impl CoreControlRegisters {
    /// Own the root's core-control partition.
    pub fn new(peripherals: CoreControlPeripherals) -> Self {
        let CoreControlPeripherals {
            pmu_cpu_stall,
            hp_sys_core_stall_status,
            hp_sys_clkrst_core,
            lp_aon_clkrst_core_reset,
            lp_sys_core_control,
        } = peripherals;
        Self {
            stall: pmu_cpu_stall,
            status: hp_sys_core_stall_status,
            clocks: hp_sys_clkrst_core,
            reset: lp_aon_clkrst_core_reset,
            system: lp_sys_core_control,
        }
    }
}

impl Uart0ClockRegisters {
    /// Own the root's UART0 clock partition.
    pub fn new(peripherals: Uart0ClockPeripherals) -> Self {
        Self(peripherals.hp_sys_clkrst_uart0)
    }
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>
    CoreControlRegisters<P, Q, R, S, T>
{
    /// Make the selected secondary's ROM jump to `entry`; zero keeps it waiting in the ROM.
    ///
    /// SOURCE: pinned rev0 ROM `ets_set_appcpu_boot_addr` (Blobray
    /// function-records): one whole-word store of the argument, then return.
    pub(crate) fn set_boot_address(&mut self, hart: SecondaryHart, entry: u32) {
        self.system.modify_boot_addr_hp_core(hart.index(), |word| {
            word.set_boot_addr_hp_core(entry);
        });
    }

    /// Request the HP system reset without acknowledging LP wakeup status.
    ///
    /// SOURCE: pinned rev0 ROM `software_reset` (Blobray function-records)
    /// reads SYS_CTRL, sets SYS_SW_RST and writes it back. Unlike that ROM
    /// write, this trigger writes zero to the WTC wakeup flag, preserving
    /// the event rather than acknowledging it. ESP-IDF 4d80c1b2
    /// `components/soc/esp32s31/register/soc/lp_system_reg.h:47-120` defines
    /// the WT, WTC and R/W lanes; ordinary fields keep the value read.
    pub(crate) fn request_system_reset(&mut self) {
        self.system.trigger_sys_ctrl_sys_sw_rst();
    }

    /// Request that `core` stall, keeping the other core's request.
    pub fn request_stall(&mut self, core: Hart) {
        self.set_code(core, STALL);
    }

    /// Request that `core` run, keeping the other core's request.
    pub fn request_run(&mut self, core: Hart) {
        self.set_code(core, RUN);
    }

    fn set_code(&mut self, core: Hart, code: u8) {
        self.stall.modify_cpu_stall_sw(|request| {
            request.set_hpcore_sw_stall_code(core.index(), code);
        });
    }

    /// Whether `core`'s stall request asks it to stall: the request, not the
    /// hardware's confirmation ([`is_stalled`](Self::is_stalled)).
    pub fn stall_requested(&self, core: Hart) -> bool {
        let request = self.stall.read_cpu_stall_sw();
        let code = request.hpcore_sw_stall_code(core.index());
        code == STALL
    }

    /// Whether the hardware reports `core` stalled.
    pub fn is_stalled(&self, core: Hart) -> bool {
        let status = self.status.read_hp_cpu_corestalled_st();
        status.hp_reg_core_corestalled_st(core.index())
    }

    /// Whether the selected HP core is held: its CPU clock gated and its global reset
    /// asserted, as ESP-IDF's single-core start leaves it.
    pub fn held(&self, core: Hart) -> bool {
        let control = self.clocks.read_hpcore_ctrl0(core.index());
        control.reg_core_global_rst_en() && !control.reg_core_cpu_clk_en()
    }

    /// Hold the selected secondary HP core: gate its CPU and CLIC clocks and
    /// assert its global reset, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `components/esp_system/port/cpu_start.c`
    /// (the single-core start) and
    /// `components/hal/esp32s31/include/hal/cpu_utility_ll.h`.
    pub fn hold(&mut self, core: SecondaryHart) {
        self.clocks
            .modify_hpcore_ctrl0(core.hart().index(), |control| {
                control
                    .set_reg_core_cpu_clk_en(false)
                    .set_reg_core_clic_clk_en(false)
                    .set_reg_core_global_rst_en(true);
            });
    }

    /// Open the selected secondary's CPU clock, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF fff9895c
    /// `components/hal/esp32s31/include/hal/cpu_utility_ll.h:63-67`
    /// (`cpu_utility_ll_enable_clock_and_reset_app_cpu`).
    pub(crate) fn enable_cpu_clock(&mut self, core: SecondaryHart) {
        self.clocks
            .modify_hpcore_ctrl0(core.hart().index(), |control| {
                control.set_reg_core_cpu_clk_en(true);
            });
    }

    /// Open the selected secondary's CLIC clock, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF fff9895c
    /// `components/hal/esp32s31/include/hal/cpu_utility_ll.h:69-71`.
    pub(crate) fn enable_clic_clock(&mut self, core: SecondaryHart) {
        self.clocks
            .modify_hpcore_ctrl0(core.hart().index(), |control| {
                control.set_reg_core_clic_clk_en(true);
            });
    }

    /// Release the selected secondary's global reset, keeping every other bit.
    ///
    /// SOURCE: ESP-IDF fff9895c
    /// `components/hal/esp32s31/include/hal/cpu_utility_ll.h:73-75`.
    pub(crate) fn release_reset(&mut self, core: SecondaryHart) {
        self.clocks
            .modify_hpcore_ctrl0(core.hart().index(), |control| {
                control.set_reg_core_global_rst_en(false);
            });
    }

    /// The selected secondary's ROM boot word.
    pub(crate) fn boot_address(&self, hart: SecondaryHart) -> u32 {
        self.system
            .read_boot_addr_hp_core(hart.index())
            .boot_addr_hp_core()
    }

    /// Reset `core`'s CPU once: a pulse, not a hold.
    pub fn reset_core(&mut self, core: Hart) {
        self.reset
            .trigger_hpcore_reset_ctrl_hpcore_sw_reset(core.index());
    }
}

impl<P: Placement> Uart0ClockRegisters<P> {
    /// Return UART0's function clock to its power-on state, XTAL undivided
    /// with its gate open, keeping the bus clock and reset bits: the boot
    /// ROM prints through UART0 after a software reset, and the source the
    /// firmware chose may not run any more.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2
    /// `components/soc/esp32s31/register/soc/hp_sys_clkrst_reg.h:1408-1446`
    /// (UART0 source, gate and divider fields) and
    /// `components/esp_hal_uart/esp32s31/include/hal/uart_ll.h`
    /// (`uart_ll_set_sclk`: XTAL selector zero).
    pub fn restore_uart0_clock(&mut self) {
        self.0.modify_uart0_ctrl0(|clock| {
            clock
                .set_reg_uart0_clk_src_sel(UART0_XTAL)
                .set_reg_uart0_sclk_div_num(UART0_UNDIVIDED)
                .set_reg_uart0_sclk_div_numerator(UART0_UNDIVIDED)
                .set_reg_uart0_sclk_div_denominator(UART0_UNDIVIDED)
                .set_reg_uart0_clk_en(true);
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
        oer_mmio::Host<'a>,
    >
{
    /// Each block over its own host memory from its instance's base.
    pub fn host(
        stall: &'a mut [u32],
        status: &'a mut [u32],
        clocks: &'a mut [u32],
        reset: &'a mut [u32],
        system: &'a mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            stall: PmuCpuStall::host(stall)?,
            status: HpSysCoreStallStatus::host(status)?,
            clocks: HpSysClkrstCore::host(clocks)?,
            reset: LpAonClkrstCoreReset::host(reset)?,
            system: LpSysCoreControl::host(system)?,
        })
    }
}

/// The bytes of host memory each block needs, in the order
/// [`CoreControlRegisters::host`] takes them.
#[cfg(any(test, feature = "test-support"))]
pub const HOST_SIZES: [usize; 5] = [
    instances::pmu::SIZE,
    instances::hp_sys::SIZE,
    instances::hp_sys_clkrst::SIZE,
    instances::lp_aon_clkrst::SIZE,
    instances::lp_sys::SIZE,
];

#[cfg(any(test, feature = "test-support"))]
impl<'a> Uart0ClockRegisters<oer_mmio::Host<'a>> {
    /// The block over host memory from its instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        HpSysClkrstUart0::host(words).map(Self)
    }
}

/// The bytes of host memory UART0's clock block needs.
#[cfg(any(test, feature = "test-support"))]
pub const UART0_HOST_SIZE: usize = instances::hp_sys_clkrst::SIZE;

// These implementations expose only the existing typed atoms to family policy.
// The containment port can only clear an entry; the launch port, whose release
// writes a nonzero entry, is reached only through `Cores::start_secondary`.
impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>
    oer_espressif_boot::drivers::cores::Registers for CoreControlRegisters<P, Q, R, S, T>
{
    type Secondary = SecondaryHart;
    const SECONDARIES: &'static [SecondaryHart] = &Hart::SECONDARIES;
    fn request_stall(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.request_stall(hart.hart());
    }
    fn hold(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.hold(hart);
    }
    fn clear_boot_address(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.set_boot_address(hart, 0);
    }
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>
    oer_espressif_boot::drivers::cores::RuntimeRegisters for CoreControlRegisters<P, Q, R, S, T>
{
    type Hart = Hart;
    type Stops = [Option<oer_espressif_boot::drivers::cores::Stopped>; super::HARTS];
    const HARTS: &'static [Hart] = &Hart::ALL;
    const BOOT: Hart = super::BOOT_HART;
    fn hart(secondary: SecondaryHart) -> Hart {
        secondary.hart()
    }
    fn save_stop(
        stops: &mut Self::Stops,
        hart: Hart,
        stopped: oer_espressif_boot::drivers::cores::Stopped,
    ) {
        *hart.at_mut(stops) = Some(stopped);
    }
    fn fence(&self, _: oer_espressif_boot::Access<'_>) {
        oer_riscv::fence();
    }
    fn held(&self, _: oer_espressif_boot::Access<'_>, hart: Hart) -> bool {
        self.held(hart)
    }
    fn is_stalled(&self, _: oer_espressif_boot::Access<'_>, hart: Hart) -> bool {
        self.is_stalled(hart)
    }
    fn request_peer_stall(&mut self, _: oer_espressif_boot::Access<'_>, hart: Hart) {
        self.request_stall(hart);
    }
    fn request_run(&mut self, _: oer_espressif_boot::Access<'_>, hart: Hart) {
        self.request_run(hart);
    }
    fn reset_core(&mut self, _: oer_espressif_boot::Access<'_>, hart: Hart) {
        self.reset_core(hart);
    }
}

impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement>
    oer_espressif_boot::drivers::cores::LaunchRegisters for CoreControlRegisters<P, Q, R, S, T>
{
    fn enable_cpu_clock(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.enable_cpu_clock(hart);
    }
    fn enable_clic_clock(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.enable_clic_clock(hart);
    }
    fn release_reset(&mut self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) {
        self.release_reset(hart);
    }
    fn set_boot_address(
        &mut self,
        _: oer_espressif_boot::Access<'_>,
        hart: SecondaryHart,
        entry: u32,
    ) {
        self.set_boot_address(hart, entry);
    }
    fn boot_address(&self, _: oer_espressif_boot::Access<'_>, hart: SecondaryHart) -> u32 {
        self.boot_address(hart)
    }
}
