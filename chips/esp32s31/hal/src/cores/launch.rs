//! A secondary hart's release: the runtime root's one-shot [`LaunchPermit`]
//! and the checked steps of [`Cores::start_secondary`](super::Cores::start_secondary).
//!
//! The permit holds what nothing else may touch while the hart starts: hart
//! 1's interrupt grant (its interrupt-matrix routes and its own CLIC's
//! parts), the retained wake-stub word the ROM consults on a secondary entry
//! and an observer of the cache's bus gates. Only
//! [`Root::into_runtime`](crate::root::Root::into_runtime) mints it, once.
//! The start consumes it; every refusal before the release leaves the hart
//! held and its boot word zero. The released hart's grant moves out once
//! ([`Released::take_interrupts`]): the staged runtime hands it to the hart,
//! which alone makes its controller from it.

use oer_esp32s31_pac::{
    cache::CacheBusObserver,
    clic::{Clic, ClicObserver},
    instances,
    interrupt_core::{self, InterruptCore1},
    lp_system_reg::LpWakeStub,
    ownership::SecondaryLaunchPeripherals,
};
use oer_espressif_boot::drivers::cores::{
    self as driver, BootWordKept, CoreState, STALL_POLLS, Unconfirmed,
};
use oer_espressif_platform_port::SecondaryEntry;
use oer_memory::ClaimCell;
use oer_mmio::{Bits, Fixed, Index, Placement};

use super::{CoresError, Hart, SecondaryHart, operate};
use crate::{
    peripherals::reset_cause::{ResetCause, ResetCauses},
    platform::{InterruptGrant, core_control::CoreControlRegisters},
};

/// The runtime root's one-shot authority to start a secondary hart. Not
/// `Clone`; only the root mints it.
#[must_use = "dropping the permit permanently loses hart 1's routes"]
pub struct LaunchPermit<
    M: Placement = Fixed<{ instances::interrupt_core1::BASE }>,
    L: Placement = Fixed<{ instances::lp_sys::BASE }>,
    C: Placement = Fixed<{ instances::cache::BASE }>,
    K: Placement = Fixed<{ instances::clic::BASE }>,
> {
    interrupts: InterruptGrant<InterruptCore1<M>, K>,
    wake_stub: LpWakeStub<L>,
    buses: CacheBusObserver<C>,
}

impl LaunchPermit {
    /// The root's permit, from its one secondary-launch partition and hart
    /// 1's CLIC parts.
    pub(crate) fn new(
        peripherals: SecondaryLaunchPeripherals,
        clic: Clic,
        clic_observer: ClicObserver,
    ) -> Self {
        let SecondaryLaunchPeripherals {
            interrupt_core1,
            lp_wake_stub,
            cache_bus_observer,
        } = peripherals;
        Self {
            interrupts: InterruptGrant::new(interrupt_core1, clic, Some(clic_observer)),
            wake_stub: lp_wake_stub,
            buses: cache_bus_observer,
        }
    }
}

/// A released secondary hart: its interrupt grant until the staged runtime
/// hands it to the hart, and what the release disconnected.
#[must_use = "dropping the released hart permanently loses its routes"]
pub struct Released<
    M: Placement = Fixed<{ instances::interrupt_core1::BASE }>,
    K: Placement = Fixed<{ instances::clic::BASE }>,
> {
    hart: SecondaryHart,
    /// Unwritten since the release, until the hart's controller routes.
    interrupts: Option<InterruptGrant<InterruptCore1<M>, K>>,
    disconnected: u16,
    first: Option<u16>,
}

impl<M: Placement, K: Placement> Released<M, K> {
    /// The released hart.
    pub const fn hart(&self) -> SecondaryHart {
        self.hart
    }

    /// How many routes of the hart's bank were connected and disconnected
    /// before the release, and the first such source.
    pub const fn disconnected(&self) -> (u16, Option<u16>) {
        (self.disconnected, self.first)
    }

    /// Move the hart's interrupt grant out, once: `None` after. Only the
    /// hart can make a controller from it
    /// ([`InterruptGrant::into_controller`]).
    pub fn take_interrupts(&mut self) -> Option<InterruptGrant<InterruptCore1<M>, K>> {
        self.interrupts.take()
    }
}

/// Why a secondary hart did not start, with what was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    /// The entry names no secondary hart of the chip.
    NotSecondary { hart: u8 },
    /// The core-control words are not installed or a fatal path holds them.
    Cores(CoresError),
    /// The hart was not held in reset with its clock gated.
    NotHeld { state: CoreState },
    /// The hart's instruction bus or the shared data bus is shut to the cache.
    FetchBusShut { ibus1: bool, dbus0: bool },
    /// After a PMU power-down reset of HP core 0 the ROM would send the hart
    /// to the light-sleep wake stub instead of its boot word.
    WakeStubArmed { cause: u8, store8: u32 },
    /// The hart still reported stalled after its run request.
    Unconfirmed,
    /// The boot word read back nonzero after its clear.
    BootWordKept { readback: u32 },
}

impl StartError {
    /// The step's code and its observations.
    pub const fn observations(self) -> [u32; 3] {
        match self {
            Self::NotSecondary { hart } => [1, hart as u32, 0],
            Self::Cores(CoresError::NotInstalled) => [2, 0, 0],
            Self::Cores(CoresError::Stopped) => [2, 1, 0],
            Self::Cores(CoresError::Unconfirmed) => [2, 2, 0],
            Self::NotHeld { state } => [
                3,
                match state {
                    CoreState::Held => 0,
                    CoreState::Stalled => 1,
                    CoreState::Released => 2,
                },
                0,
            ],
            Self::FetchBusShut { ibus1, dbus0 } => [4, ibus1 as u32, dbus0 as u32],
            Self::WakeStubArmed { cause, store8 } => [5, cause as u32, store8],
            Self::Unconfirmed => [6, 0, 0],
            Self::BootWordKept { readback } => [7, readback, 0],
        }
    }
}

/// HP core 0's reset causes after which the rev0 ROM consults the wake stub
/// on a secondary entry: 5 (PMU HP system power-down) and 6 (PMU CPU
/// power-down).
///
/// SOURCE: ESP_IDF_4D80C1B2_S31_ROM_RTC_SLEEP_WAKE_STUB and
/// ROM_REV0_SECONDARY_WAKE_STUB in `chips/esp32s31/registers/evidence/lp-system.toml`.
const fn wake_stub_cause(cause: ResetCause) -> bool {
    matches!(cause, ResetCause::CorePowerDown | ResetCause::CpuPowerDown)
}

/// The secondary hart an entry names.
fn secondary(entry: SecondaryEntry) -> Result<SecondaryHart, StartError> {
    Hart::SECONDARIES
        .into_iter()
        .find(|secondary| secondary.hart().get() == usize::from(entry.hart()))
        .ok_or(StartError::NotSecondary { hart: entry.hart() })
}

/// The ordered start over `cell`'s words; `clear_printf_slots` empties the
/// ROM's two output slots before the release.
pub(super) fn start_with<
    P: Placement,
    Q: Placement,
    R: Placement,
    S: Placement,
    T: Placement,
    M: Placement,
    L: Placement,
    C: Placement,
    K: Placement,
>(
    cell: &ClaimCell<CoreControlRegisters<P, Q, R, S, T>>,
    permit: LaunchPermit<M, L, C, K>,
    causes: ResetCauses,
    entry: SecondaryEntry,
    clear_printf_slots: impl FnOnce(),
) -> Result<Released<M, K>, StartError> {
    let hart = secondary(entry)?;
    let LaunchPermit {
        mut interrupts,
        wake_stub,
        buses,
    } = permit;
    let routes = interrupts.bank();
    // (1) The hart is held, as containment and every fatal reset leave it.
    let state = operate(cell, |words| driver::state(words, hart)).map_err(StartError::Cores)?;
    if state != CoreState::Held {
        return Err(StartError::NotHeld { state });
    }
    // (2) Its instruction bus and the shared data bus reach the cache: the
    // bootstrap opened them for the image's declared harts.
    let ibus1 = buses.read_l1_icache_ctrl().l1_icache_shut_ibus1();
    let dbus0 = buses.read_l1_dcache_ctrl().l1_dcache_shut_dbus0();
    if ibus1 || dbus0 {
        return Err(StartError::FetchBusShut { ibus1, dbus0 });
    }
    // (3) The ROM takes its boot-word path, not the light-sleep wake stub:
    // after cause 5 or 6 of HP core 0 it jumps to bits 31:2 of LP_STORE8
    // when bit 0 (deep sleep) is clear and the address is nonzero.
    let cause = causes.of(Hart::H0);
    let store8 = wake_stub.read_lp_store8().lp_scratch8();
    if wake_stub_cause(cause) && store8 & 1 == 0 && store8 & !3 != 0 {
        return Err(StartError::WakeStubArmed {
            cause: cause.code(),
            store8,
        });
    }
    // (4) No source reaches the hart before an owner routes it.
    let mut disconnected = 0_u16;
    let mut first = None;
    for source in 0..interrupt_core::route::COUNT {
        let Ok(index) = Index::new(source) else {
            continue;
        };
        if routes.read_route(index).map().get() != 0 {
            disconnected = disconnected.saturating_add(1);
            first = first.or(u16::try_from(source).ok());
            routes.modify_route(index, |route| {
                route.set_map(Bits::constant::<0>());
            });
        }
    }
    oer_riscv::fence();
    // (5) The ROM's secondary path prints through neither of its slots.
    clear_printf_slots();
    // (6) Release, the boot word last.
    operate(cell, |words| {
        driver::release(words, hart, entry.address(), STALL_POLLS)
    })
    .map_err(StartError::Cores)?
    .map_err(|Unconfirmed| StartError::Unconfirmed)?;
    Ok(Released {
        hart,
        interrupts: Some(interrupts),
        disconnected,
        first,
    })
}

/// Clear the released hart's boot word over `cell`'s words.
pub(super) fn retire_with<
    P: Placement,
    Q: Placement,
    R: Placement,
    S: Placement,
    T: Placement,
    M: Placement,
    K: Placement,
>(
    cell: &ClaimCell<CoreControlRegisters<P, Q, R, S, T>>,
    released: &mut Released<M, K>,
) -> Result<(), StartError> {
    operate(cell, |words| {
        driver::retire_boot_address(words, released.hart)
    })
    .map_err(StartError::Cores)?
    .map_err(|BootWordKept { readback }| StartError::BootWordKept { readback })
}

#[cfg(test)]
impl<'a>
    LaunchPermit<oer_mmio::Host<'a>, oer_mmio::Host<'a>, oer_mmio::Host<'a>, oer_mmio::Host<'a>>
{
    /// The permit over host memory: hart 1's route bank, LP_SYS, the cache
    /// and hart 1's CLIC.
    pub(super) fn host(
        routes: &'a mut [u32],
        lp_sys: &'a mut [u32],
        cache: &'a mut [u32],
        clic: &'a mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        let (buses, _cache_boot) = oer_esp32s31_pac::cache::cache_host(cache)?;
        let (lines, config) = oer_esp32s31_pac::clic::clic_host(clic)?;
        Ok(Self {
            interrupts: InterruptGrant::new(InterruptCore1::host(routes)?, lines, Some(config)),
            wake_stub: LpWakeStub::host(lp_sys)?,
            buses,
        })
    }
}
