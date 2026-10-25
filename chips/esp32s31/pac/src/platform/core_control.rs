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

use oer_mmio::{Fixed, Placement};

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
    pub stall: PmuCpuStall<P>,
    pub status: HpSysCoreStallStatus<Q>,
    pub clocks: HpSysClkrstCore<R>,
    pub reset: LpAonClkrstCoreReset<S>,
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
    pub HpSysClkrstUart0<P>,
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
