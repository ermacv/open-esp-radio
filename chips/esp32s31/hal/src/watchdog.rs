//! The chip's watchdogs: both timer groups' main watchdogs, the RTC
//! watchdog and the super watchdog, owned from the reset root's
//! `WatchdogPeripherals`.
//!
//! The bootstrap takes the RTC, super and timer group 0's main watchdogs
//! over from the bootloader (`oer_espressif_boot::watchdog`); stage two
//! owns all four ([`Watchdogs`]). Timer group 1's main watchdog serves the
//! system deadline.

mod registers;
pub use oer_espressif_watchdog::{
    MainSession, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog, StoppedMainWatchdog,
    SuperWatchdog,
    deadline::{DeadlineBudget, DeadlineError},
};
pub use registers::{Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters};

use oer_esp32s31_pac::ownership::WatchdogPeripherals;

/// The main watchdogs' microsecond tick on the chip's 40 MHz XTAL, their
/// clock source.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/soc/esp32s31/include/soc/clk_tree_defs.h:125-127`
/// (the chip's XTAL is 40 MHz) and
/// `components/esp_hal_wdt/esp32s31/include/hal/mwdt_ll.h:273-283`
/// (`mwdt_ll_set_clock_source`: XTAL).
pub const MAIN_WATCHDOG_TICK: MicrosecondTick = match MicrosecondTick::from_source_hz(40_000_000) {
    Some(tick) => tick,
    None => panic!("40 MHz is a whole number of megahertz"),
};

/// The watchdogs' register owners, split from the root's partition.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct WatchdogRegisters {
    /// Timer group 0's main watchdog.
    pub main0: Mwdt0Registers,
    /// Timer group 1's main watchdog and its clock and reset word.
    pub main1: Mwdt1Registers,
    /// The RTC watchdog.
    pub rtc: RwdtRegisters,
    /// The super watchdog.
    pub super_watchdog: SwdRegisters,
}

impl WatchdogRegisters {
    /// Split the root's watchdog partition.
    pub fn new(peripherals: WatchdogPeripherals) -> Self {
        let WatchdogPeripherals {
            timg0_wdt,
            timg1_wdt,
            hp_sys_clkrst_timg1,
            lp_wdt,
            lp_wdt_swd,
        } = peripherals;
        Self {
            main0: Mwdt0Registers::new(timg0_wdt),
            main1: Mwdt1Registers::new(timg1_wdt, hp_sys_clkrst_timg1),
            rtc: RwdtRegisters::new(lp_wdt),
            super_watchdog: SwdRegisters::new(lp_wdt_swd),
        }
    }
}

/// Every watchdog of the chip, owned for stage two's lifetime.
pub struct Watchdogs {
    /// Timer group 0's main watchdog, only stopped: its watchdog clock is
    /// not the image's, which gates it.
    pub main0: StoppedMainWatchdog<Mwdt0Registers>,
    /// Timer group 1's main watchdog, the system deadline's timer.
    pub main1: MainWatchdog<Mwdt1Registers>,
    /// The RTC watchdog.
    pub rtc: RtcWatchdog<RwdtRegisters>,
    /// The super watchdog.
    pub super_watchdog: SuperWatchdog<SwdRegisters>,
}

impl Watchdogs {
    /// Own every watchdog: run timer group 1 from reset on the XTAL clock
    /// and stop both main watchdogs, latched and locked. The RTC and super
    /// watchdogs are left as they are.
    pub fn new(registers: WatchdogRegisters) -> Self {
        let WatchdogRegisters {
            main0,
            mut main1,
            rtc,
            super_watchdog,
        } = registers;
        main1.start_clock();
        Self {
            main0: StoppedMainWatchdog::new(main0),
            main1: MainWatchdog::new(main1, MAIN_WATCHDOG_TICK),
            rtc: RtcWatchdog::new(rtc),
            super_watchdog: SuperWatchdog::new(super_watchdog),
        }
    }
}

/// The system deadline on timer group 1's main watchdog, measured on `C`.
pub type DeadlineWatchdog<C> =
    oer_espressif_watchdog::deadline::DeadlineWatchdog<Mwdt1Registers, C>;

/// An armed system deadline on timer group 1's main watchdog.
pub type DeadlineLease<C> = oer_espressif_watchdog::deadline::DeadlineLease<Mwdt1Registers, C>;
