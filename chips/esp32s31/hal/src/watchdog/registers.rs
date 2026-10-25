//! The watchdog owners over the root's parts: the main watchdogs of both
//! timer groups (MWDT0, MWDT1), timer group 1's clock and reset word, the
//! RTC watchdog (RWDT) and the super watchdog (SWD).
//!
//! Each owner implements the `oer-espressif-watchdog` ports with one
//! read-modify-write of its generated fields per call, keeping every other
//! bit, and strobes only through the generated triggers.
//!
//! ```compile_fail,E0061
//! # use oer_esp32s31_hal::watchdog::Mwdt1Registers;
//! # use oer_espressif_watchdog::{WRITE_KEY, WriteProtected};
//! fn raw_key(mut mwdt1: Mwdt1Registers) {
//!     mwdt1.store_write_key(WRITE_KEY); // the key is stored only inside a session
//! }
//! ```
//!
//! Timer group 0's watchdog clock is not the image's, so its watchdog is
//! only stopped, never armed:
//!
//! ```compile_fail,E0277
//! # use oer_esp32s31_hal::watchdog::Mwdt0Registers;
//! # use oer_espressif_watchdog::{MainWatchdog, MicrosecondTick};
//! fn arm(mwdt0: Mwdt0Registers, tick: MicrosecondTick) {
//!     let _armable = MainWatchdog::new(mwdt0, tick);
//! }
//! ```

use oer_espressif_watchdog::{
    Access, MainConfig, MainStageAction, MainWatchdogEnable, MainWatchdogRegisters, ResetLength,
    RtcStageAction, RtcWatchdogArm, RtcWatchdogRegisters, SuperWatchdogRegisters, WriteProtected,
};
use oer_mmio::{Bits, Fixed, Placement};

use oer_esp32s31_pac::{
    hp_sys_clkrst::HpSysClkrstTimg1,
    instances,
    rtc_wdt::{LpWdt, LpWdtSwd},
    timg::{Timg0Wdt, Timg1Wdt},
};

#[cfg(test)]
#[path = "register_tests.rs"]
mod tests;

/// Timer group 0's main watchdog.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt0Registers<P: Placement = Fixed<{ instances::timg0::BASE }>>(Timg0Wdt<P>);

/// Timer group 1's main watchdog and its group's clock and reset word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt1Registers<
    P: Placement = Fixed<{ instances::timg1::BASE }>,
    Q: Placement = Fixed<{ instances::hp_sys_clkrst::BASE }>,
> {
    wdt: Timg1Wdt<P>,
    clock: HpSysClkrstTimg1<Q>,
}

/// The RTC watchdog.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct RwdtRegisters<P: Placement = Fixed<{ instances::lp_wdt::BASE }>>(LpWdt<P>);

/// The super watchdog.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct SwdRegisters<P: Placement = Fixed<{ instances::lp_wdt::BASE }>>(LpWdtSwd<P>);

/// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:273-301`: the watchdog source XTAL.
const XTAL_SOURCE: Bits<2> = Bits::constant::<0>();
/// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:219-222`: any store feeds, the LL writes one.
const FEED: u32 = 1;

impl<P: Placement> Mwdt0Registers<P> {
    pub(super) fn new(wdt: Timg0Wdt<P>) -> Self {
        Self(wdt)
    }
}

impl<P: Placement, Q: Placement> Mwdt1Registers<P, Q> {
    pub(super) fn new(wdt: Timg1Wdt<P>, clock: HpSysClkrstTimg1<Q>) -> Self {
        Self { wdt, clock }
    }

    /// Run timer group 1 from reset with its watchdog on the XTAL clock:
    /// bus clock on, a reset pulse, the watchdog's source XTAL and its
    /// clock on.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/esp_hal_wdt/esp32s31/include/hal/mwdt_ll.h:273-301`
    /// (XTAL source and module gate),
    /// `components/soc/esp32s31/register/soc/hp_sys_clkrst_reg.h`
    /// (`TIMERGRP1_CTRL0`: APB clock, reset, watchdog gate/source) and
    /// `components/esp_hal_timg/esp32s31/include/hal/timg_ll.h`
    /// (`timg_ll_enable_bus_clock`, `timg_ll_reset_register`).
    pub fn start_clock(&mut self) {
        self.clock.modify_timergrp1_ctrl0(|w| {
            w.set_reg_timergrp1_apb_clk_en(true);
        });
        self.clock.modify_timergrp1_ctrl0(|w| {
            w.set_reg_timergrp1_rst_en(true);
        });
        self.clock.modify_timergrp1_ctrl0(|w| {
            w.set_reg_timergrp1_rst_en(false);
        });
        self.clock.modify_timergrp1_ctrl0(|w| {
            w.set_reg_timergrp1_wdt_src_sel(XTAL_SOURCE)
                .set_reg_timergrp1_wdt_clk_en(true);
        });
    }
}

impl<P: Placement> RwdtRegisters<P> {
    pub(super) fn new(wdt: LpWdt<P>) -> Self {
        Self(wdt)
    }
}

impl<P: Placement> SwdRegisters<P> {
    pub(super) fn new(swd: LpWdtSwd<P>) -> Self {
        Self(swd)
    }
}

/// A main watchdog stage's two-bit action field.
/// SOURCE: ESP-IDF 4d59230d `timer_group_reg.h:464-470` (`TIMG_WDT_STG0`).
/// SOURCE: ESP-IDF 4d59230d `rtc_wdt_reg.h` (`RTC_WDT_STG0`: 4 the RTC
/// reset).
const fn rtc_stage(action: RtcStageAction) -> Bits<3> {
    match action {
        RtcStageAction::ResetRtc => Bits::constant::<4>(),
    }
}

const fn main_stage(action: MainStageAction) -> Bits<2> {
    match action {
        MainStageAction::Off => Bits::constant::<0>(),
        MainStageAction::Interrupt => Bits::constant::<1>(),
        MainStageAction::ResetCpu => Bits::constant::<2>(),
        MainStageAction::ResetSystem => Bits::constant::<3>(),
    }
}

/// A reset length's three-bit selector field.
/// SOURCE: ESP-IDF 4d59230d `timer_group_reg.h:388-425` and `wdt_hal_iram.c:58-59,79-80`.
const fn reset_length(length: ResetLength) -> Bits<3> {
    match length {
        ResetLength::Longest => Bits::constant::<7>(),
    }
}

/// The write-protect and stop ports over one timer group's typed watchdog
/// part.
macro_rules! main_watchdog_enable {
    ([$($generics:tt)*] $owner:ty, $field:tt) => {
        impl<$($generics)*> WriteProtected for $owner {
            fn store_write_key(&mut self, _: Access<'_>, key: u32) {
                self.$field.modify_wdtwprotect(|w| {
                    w.set_wdt_wkey(key);
                });
            }
        }

        impl<$($generics)*> MainWatchdogEnable for $owner {
            fn set_running(&mut self, _: Access<'_>, enabled: bool, flash_boot: bool) {
                self.$field.modify_wdtconfig0(|w| {
                    w.set_wdt_en(enabled).set_wdt_flashboot_mod_en(flash_boot);
                });
            }

            fn latch(&mut self, _: Access<'_>) {
                self.$field.trigger_wdtconfig0_wdt_conf_update_en();
            }
        }
    };
}

main_watchdog_enable!([P: Placement] Mwdt0Registers<P>, 0);
main_watchdog_enable!([P: Placement, Q: Placement] Mwdt1Registers<P, Q>, wdt);

impl<P: Placement, Q: Placement> MainWatchdogRegisters for Mwdt1Registers<P, Q> {
    fn configure(&mut self, _: Access<'_>, config: MainConfig) {
        let [stage0, stage1, stage2, stage3] = config.stages;
        self.wdt.modify_wdtconfig0(|w| {
            w.set_wdt_en(config.enabled)
                .set_wdt_flashboot_mod_en(config.flash_boot)
                .set_wdt_stg0(main_stage(stage0))
                .set_wdt_stg1(main_stage(stage1))
                .set_wdt_stg2(main_stage(stage2))
                .set_wdt_stg3(main_stage(stage3))
                .set_wdt_cpu_reset_length(reset_length(config.cpu_reset_length))
                .set_wdt_sys_reset_length(reset_length(config.system_reset_length));
        });
    }

    fn set_prescale(&mut self, _: Access<'_>, prescale: u16) {
        self.wdt.modify_wdtconfig1(|w| {
            w.set_wdt_clk_prescale(prescale);
        });
    }

    fn reset_divider(&mut self, _: Access<'_>) {
        self.wdt.trigger_wdtconfig1_wdt_divcnt_rst();
    }

    fn set_stage0_timeout(&mut self, _: Access<'_>, ticks: u32) {
        self.wdt.modify_wdtconfig2(|w| {
            w.set_wdt_stg0_hold(ticks);
        });
    }

    /// SOURCE: ESP-IDF 4d59230d `components/esp_hal_wdt/esp32s31/include/hal/mwdt_ll.h:219-222` (`mwdt_ll_feed`
    /// writes one; any value feeds).
    fn feed(&mut self, _: Access<'_>) {
        self.wdt.trigger_wdtfeed_wdt_feed(FEED);
    }
}

impl<P: Placement> WriteProtected for RwdtRegisters<P> {
    fn store_write_key(&mut self, _: Access<'_>, key: u32) {
        self.0.modify_wprotect(|w| {
            w.set_wdt_wkey(key);
        });
    }
}

impl<P: Placement> RtcWatchdogRegisters for RwdtRegisters<P> {
    fn set_running(&mut self, _: Access<'_>, enabled: bool, flash_boot: bool) {
        self.0.modify_config0(|w| {
            w.set_wdt_en(enabled).set_wdt_flashboot_mod_en(flash_boot);
        });
    }
}

impl<P: Placement> RtcWatchdogArm for RwdtRegisters<P> {
    /// SOURCE: ESP-IDF 4d59230d `rwdt_ll.h:117-123` (`rwdt_ll_config_stage`
    /// stores stage 0's hold word whole).
    fn set_stage0_hold(&mut self, _: Access<'_>, hold: u32) {
        self.0.modify_config1(|w| {
            w.set_wdt_stg0_hold(hold);
        });
    }

    /// SOURCE: ESP-IDF 4d59230d `rwdt_ll.h:117-121` (`hw->config0.wdt_stg0`).
    fn set_stage0_action(&mut self, _: Access<'_>, action: RtcStageAction) {
        self.0.modify_config0(|w| {
            w.set_wdt_stg0(rtc_stage(action));
        });
    }

    /// SOURCE: ESP-IDF 4d59230d `rwdt_ll.h:265-268` (`rwdt_ll_feed` sets
    /// the one feed bit).
    fn feed(&mut self, _: Access<'_>) {
        self.0.trigger_feed_feed();
    }
}

impl<P: Placement> WriteProtected for SwdRegisters<P> {
    fn store_write_key(&mut self, _: Access<'_>, key: u32) {
        self.0.modify_swd_wprotect(|w| {
            w.set_swd_wkey(key);
        });
    }
}

impl<P: Placement> SuperWatchdogRegisters for SwdRegisters<P> {
    fn set_auto_feed(&mut self, _: Access<'_>, enabled: bool) {
        self.0.modify_swd_config(|w| {
            w.set_swd_auto_feed_en(enabled);
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> Mwdt0Registers<oer_mmio::Host<'a>> {
    /// Timer group 0's watchdog words over host memory from the group's
    /// base, of at least its instance's size.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Timg0Wdt::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b> Mwdt1Registers<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// Timer group 1's watchdog and clock words over host memory from each
    /// instance's base.
    pub fn host(wdt: &'a mut [u32], clock: &'b mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            wdt: Timg1Wdt::host(wdt)?,
            clock: HpSysClkrstTimg1::host(clock)?,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> RwdtRegisters<oer_mmio::Host<'a>> {
    /// The RTC watchdog's words over host memory from its instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpWdt::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> SwdRegisters<oer_mmio::Host<'a>> {
    /// The super watchdog's words over host memory from its instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpWdtSwd::host(words).map(Self)
    }
}
