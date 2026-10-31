//! The S31 binding: owned parts, unchanged profiles and narrow HAL adapters.
use oer_esp32s31_hal::{
    boot,
    peripherals::{system_timer, usb_serial_jtag, watchdog},
    root::{Bootstrap, Root},
};
use oer_espressif_boot::{
    drivers::system_timer::{LatchStuck, Monotonic},
    watchdog::BootWatchdogs,
};
use oer_espressif_staged_bootstrap::startup::{BootProfile, Owners};

type WatchdogParts = (
    watchdog::Mwdt0Registers,
    watchdog::RwdtRegisters,
    watchdog::SwdRegisters,
);
type Watchdogs =
    BootWatchdogs<watchdog::Mwdt0Registers, watchdog::RwdtRegisters, watchdog::SwdRegisters>;

/// The existing 160 MHz S31 cold-start profile.
pub struct S31;

impl BootProfile for S31 {
    type Port = oer_esp32s31_hal::platform::Port;
    type Root = Root;
    type Console = usb_serial_jtag::ConsoleRegisters;
    type Timer = system_timer::BootstrapTimer;
    type Monotonic =
        Monotonic<system_timer::SystimerCounter, { system_timer::TICKS_PER_MICROSECOND }>;
    type Soc = boot::Soc;
    type Brownout = boot::Brownout;
    type LpFast = boot::LpFastClock;
    type Clock = boot::ClockRegisters;
    type Active = boot::ActivePower;
    type Psram = boot::PsramRegisters;
    type Cache = boot::CacheRegisters;
    type Flash = boot::FlashRegisters;
    type WatchdogParts = WatchdogParts;
    type Watchdogs = Watchdogs;

    const BUDGET: boot::BootBudget = boot::BOOT_BUDGET;
    const LP_FAST: oer_espressif_boot::drivers::clock::LpFastProfile<boot::LpFastSource> =
        crate::LP_FAST;
    const ENTROPY: oer_espressif_boot::drivers::entropy::Profile<boot::Bits<5>, boot::Bits<9>> =
        crate::ENTROPY;
    const CPU: boot::CpuClockProfile<boot::CpuRoot, boot::BusDividers, boot::CpllDividers> =
        boot::CpuProfile::Mhz160.clock();
    const CLOCK_WAITS: boot::ClockWaits = boot::CLOCK_WAITS;
    const PSRAM_WAITS: boot::PsramWaits = boot::PSRAM_WAITS;
    const CACHE: boot::CacheGeometry<boot::CacheMap> = boot::CACHE_GEOMETRY;
    const CACHE_SYNC: boot::BootWait = boot::CACHE_SYNC;
    const TUNING_WAITS: boot::TuningWaits = boot::TUNING_WAITS;
    const SECONDARY_FETCH_WAITS: boot::SuspendWaits = boot::SUSPEND_WAITS;

    fn timings() -> &'static [boot::FlashTimingCandidate] {
        &boot::FLASH_TIMING_CANDIDATES
    }
    fn psram_profile(size: u32) -> boot::OctalPsramProfile<boot::PsramSupplyLevel> {
        boot::octal_psram(size)
    }
    fn flash_geometry(capacity: u32) -> boot::FlashGeometry {
        boot::flash_geometry(capacity)
    }
    fn claim() -> Option<Root> {
        Root::take()
    }
    fn split(root: Root) -> Owners<Self> {
        let Bootstrap {
            console,
            timer,
            soc,
            brownout,
            main_watchdog0,
            rtc_watchdog,
            super_watchdog,
            clock,
            lp_fast_clock,
            active_power,
            psram,
            cache,
            flash,
            flash_window,
        } = root.into_bootstrap();
        Owners {
            console,
            timer,
            soc,
            brownout,
            lp_fast: lp_fast_clock,
            clock,
            active: active_power,
            psram,
            cache,
            flash,
            flash_window,
            watchdogs: (main_watchdog0, rtc_watchdog, super_watchdog),
        }
    }
    fn install_console(console: Self::Console) -> Result<(), &'static str> {
        usb_serial_jtag::install(console).map_err(|_| "console-installed")
    }
    fn start_timer(timer: Self::Timer) -> Result<Self::Monotonic, LatchStuck> {
        Monotonic::start_initial_latch_check(timer.open_clock())
    }
    fn take_watchdogs((main0, rtc, super_watchdog): WatchdogParts) -> Watchdogs {
        BootWatchdogs::take_over(main0, rtc, super_watchdog)
    }
    fn hand_off_watchdogs(watchdogs: Watchdogs) {
        let _stopped = watchdogs.hand_off();
    }
    fn brownout(
        owner: &mut Self::Brownout,
        deadline: &boot::BootDeadline<'_, Self::Monotonic>,
    ) -> Result<(), oer_espressif_boot::drivers::brownout::BrownoutFailure> {
        crate::apply_brownout(owner, deadline)
    }
    fn loan_psram(
        mapped: oer_espressif_boot::psram::MappedPsram<boot::PsramRegisters>,
    ) -> &'static mut [u8] {
        boot::PsramRegisters::into_region(mapped)
    }
}
