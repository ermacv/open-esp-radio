//! The watchdog blocks the image takes from esp-hal: the main watchdogs of
//! both timer groups (MWDT0, MWDT1), timer group 1's clock and reset word,
//! the RTC watchdog (RWDT) and the super watchdog (SWD).
//!
//! One handoff proof, made once by the esp-hal adapter that consumed
//! esp-hal's `TIMG0`, `TIMG1` and `LP_WDT` singletons, claims the watchdog
//! partition. Each owner implements the register traits of
//! `oer-espressif-watchdog` with the typed fields, one read-modify-write per
//! call, so every other bit of a word, reserved ones included, is kept and
//! every write trigger is zero except in its own trigger. The traits' calls
//! need an `oer_espressif_watchdog::Access`, which only that crate's
//! sequences make, so an owner's words are written only through them.
//!
//! ```compile_fail,E0061
//! # use oer_esp32c5_pac::watchdog::Mwdt1Registers;
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
//! # use oer_esp32c5_pac::watchdog::Mwdt0Registers;
//! # use oer_espressif_watchdog::{MainWatchdog, MicrosecondTick};
//! fn arm(mwdt0: Mwdt0Registers, tick: MicrosecondTick) {
//!     let _armable = MainWatchdog::new(mwdt0, tick);
//! }
//! ```

use oer_espressif_watchdog::{
    Access, MainConfig, MainStageAction, MainWatchdogEnable, MainWatchdogRegisters, ResetLength,
    RtcWatchdogRegisters, SuperWatchdogRegisters, WriteProtected,
};
use oer_mmio::{Bits, Fixed, Placement};

use crate::generated::{
    lp_wdt::{self, LpWdt},
    lp_wdt_swd::{self, LpWdtSwd},
    pcr_timg1::{self, PcrTimg1},
    peripheral_ownership::{WatchdogPeripherals, WatchdogPeripheralsClaims},
    timg0_wdt::{self, Timg0Wdt},
    timg1_wdt::{self, Timg1Wdt},
};

#[cfg(test)]
mod tests;

/// Proof that the platform transferred esp-hal's `TIMG0`, `TIMG1` and
/// `LP_WDT` singletons.
#[must_use = "the watchdog owners need the consumed platform ownership"]
pub struct WatchdogHandoff(());

impl WatchdogHandoff {
    /// Assert the platform's unique, permanent watchdog handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed esp-hal's `TIMG0`, `TIMG1` and
    /// `LP_WDT` singletons, and no other code writes the watchdog words of
    /// either timer group, timer group 1's clock and reset word or the RTC
    /// and super watchdog words thereafter.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive watchdog ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Every watchdog block of the chip.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct WatchdogBlocks {
    pub mwdt0: Mwdt0Registers,
    pub mwdt1: Mwdt1Registers,
    pub rwdt: RwdtRegisters,
    pub swd: SwdRegisters,
}

impl WatchdogBlocks {
    /// Claim the watchdog blocks from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: WatchdogHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `WatchdogHandoff`'s constructor guarantees exclusive,
        // permanent ownership of the watchdog words. The generated `const`
        // assertion beside the typed blocks checks that no other typed block
        // lists them. esp-hal writes them only inside `esp_hal::init` and its
        // timer-group and RTC drivers, which need the consumed singletons;
        // reviewed in `registers/shared-words.toml`.
        let claims = unsafe { oer_mmio::assume_owned::<WatchdogPeripheralsClaims>() };
        let WatchdogPeripherals {
            timg0_wdt,
            timg1_wdt,
            pcr_timg1,
            lp_wdt,
            lp_wdt_swd,
        } = WatchdogPeripherals::from_claims(claims);
        Self {
            mwdt0: Mwdt0Registers(timg0_wdt),
            mwdt1: Mwdt1Registers {
                wdt: timg1_wdt,
                clock: pcr_timg1,
            },
            rwdt: RwdtRegisters(lp_wdt),
            swd: SwdRegisters(lp_wdt_swd),
        }
    }
}

/// Timer group 0's main watchdog. Timer group 0's bus clock is never gated
/// (esp-hal keeps it running for its RTC calibration), but its watchdog
/// clock is esp-hal's, which gates it: the owner implements only the stop
/// fields ([`MainWatchdogEnable`]), so it is never armed.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/lib.rs:782` and
/// `esp-hal/src/timer/timg.rs:670-673` (timer group 0's watchdog disabled and
/// its clock released in `esp_hal::init`).
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt0Registers<P: Placement = Fixed<{ timg0_wdt::BASE }>>(Timg0Wdt<P>);

/// Timer group 1's main watchdog with its group's clock and reset words.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt1Registers<
    P: Placement = Fixed<{ timg1_wdt::BASE }>,
    Q: Placement = Fixed<{ pcr_timg1::BASE }>,
> {
    wdt: Timg1Wdt<P>,
    clock: PcrTimg1<Q>,
}

impl<P: Placement, Q: Placement> Mwdt1Registers<P, Q> {
    /// Run timer group 1 from reset with its watchdog on the XTAL clock:
    /// bus clock on, a reset pulse, the watchdog's source XTAL and its
    /// clock on. `esp_hal::init` gates the group's bus clock.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:188-203`
    /// (`TimerGroup::new`: `PeripheralClockControl::enable` then `reset`),
    /// `esp-metadata-generated/src/_generated_esp32c5.rs:5108-5113,5214-5219`
    /// (`TG1_CLK_EN`, `TG1_RST_EN`) and `esp-hal/src/soc/esp32c5/clocks.rs:668-702`
    /// (`TG1_WDT_CLK_EN`, `TG1_WDT_CLK_SEL` 0 = XTAL), as ESP-IDF 4d59230d
    /// `components/esp_hal_wdt/esp32c5/include/hal/mwdt_ll.h:273-313`.
    pub fn start_clock(&mut self) {
        self.clock.modify_timergroup1_conf(|w| {
            w.set_tg1_clk_en(true);
        });
        self.clock.modify_timergroup1_conf(|w| {
            w.set_tg1_rst_en(true);
        });
        self.clock.modify_timergroup1_conf(|w| {
            w.set_tg1_rst_en(false);
        });
        self.clock.modify_timergroup1_wdt_clk_conf(|w| {
            w.set_tg1_wdt_clk_sel(Bits::constant::<0>())
                .set_tg1_wdt_clk_en(true);
        });
    }
}

/// The RTC watchdog's configuration and write-protect words.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct RwdtRegisters<P: Placement = Fixed<{ lp_wdt::BASE }>>(LpWdt<P>);

/// The super watchdog's configuration and write-protect words.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct SwdRegisters<P: Placement = Fixed<{ lp_wdt_swd::BASE }>>(LpWdtSwd<P>);

/// A main watchdog stage's two-bit action field.
const fn main_stage(action: MainStageAction) -> Bits<2> {
    match action {
        MainStageAction::Off => Bits::constant::<0>(),
        MainStageAction::Interrupt => Bits::constant::<1>(),
        MainStageAction::ResetCpu => Bits::constant::<2>(),
        MainStageAction::ResetSystem => Bits::constant::<3>(),
    }
}

/// A reset length's three-bit selector field.
const fn reset_length(length: ResetLength) -> Bits<3> {
    match length {
        ResetLength::Longest => Bits::constant::<7>(),
    }
}

/// The write-protect and stop traits over one timer group's typed watchdog
/// block.
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

    /// SOURCE: ESP-IDF 4d59230d `components/esp_hal_wdt/esp32c5/include/hal/mwdt_ll.h:219-222` (`mwdt_ll_feed`
    /// writes one; any value feeds).
    fn feed(&mut self, _: Access<'_>) {
        self.wdt.write_wdtfeed(|w| {
            w.set_wdt_feed(1);
        });
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
    /// Timer group 0's watchdog words over host memory of at least
    /// [`timg0_wdt::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Timg0Wdt::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b> Mwdt1Registers<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// Timer group 1's watchdog and clock words over host memory of at
    /// least [`timg1_wdt::SIZE`] and [`pcr_timg1::SIZE`] bytes.
    pub fn host(wdt: &'a mut [u32], clock: &'b mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            wdt: Timg1Wdt::host(wdt)?,
            clock: PcrTimg1::host(clock)?,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> RwdtRegisters<oer_mmio::Host<'a>> {
    /// The RTC watchdog's words over host memory of at least
    /// [`lp_wdt::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpWdt::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> SwdRegisters<oer_mmio::Host<'a>> {
    /// The super watchdog's words over host memory of at least
    /// [`lp_wdt_swd::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpWdtSwd::host(words).map(Self)
    }
}
