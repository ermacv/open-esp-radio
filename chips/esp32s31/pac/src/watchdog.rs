//! Watchdog acquisition and ownership parts; operations live in the HAL.

use oer_mmio::{Fixed, Placement};

use crate::generated::{
    hp_sys_clkrst_timg1::{self, HpSysClkrstTimg1},
    lp_wdt::{self, LpWdt},
    lp_wdt_swd::{self, LpWdtSwd},
    peripheral_ownership::{WatchdogPeripherals, WatchdogPeripheralsClaims},
    timg0_wdt::{self, Timg0Wdt},
    timg1_wdt::{self, Timg1Wdt},
};

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
            hp_sys_clkrst_timg1,
            lp_wdt,
            lp_wdt_swd,
        } = WatchdogPeripherals::from_claims(claims);
        Self {
            mwdt0: Mwdt0Registers(timg0_wdt),
            mwdt1: Mwdt1Registers {
                wdt: timg1_wdt,
                clock: hp_sys_clkrst_timg1,
            },
            rwdt: RwdtRegisters(lp_wdt),
            swd: SwdRegisters(lp_wdt_swd),
        }
    }
}

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt0Registers<P: Placement = Fixed<{ timg0_wdt::BASE }>>(pub Timg0Wdt<P>);

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Mwdt1Registers<
    P: Placement = Fixed<{ timg1_wdt::BASE }>,
    Q: Placement = Fixed<{ hp_sys_clkrst_timg1::BASE }>,
> {
    pub wdt: Timg1Wdt<P>,
    pub clock: HpSysClkrstTimg1<Q>,
}

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct RwdtRegisters<P: Placement = Fixed<{ lp_wdt::BASE }>>(pub LpWdt<P>);

#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct SwdRegisters<P: Placement = Fixed<{ lp_wdt_swd::BASE }>>(pub LpWdtSwd<P>);
