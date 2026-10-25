//! The chip's watchdogs: both timer groups' main watchdogs, the RTC
//! watchdog and the super watchdog, owned from their platform handoff.
//!
//! `esp_hal::init` still stops the RTC and super watchdogs and timer group
//! 0's main watchdog in every image; [`Watchdogs`] owns them so nothing
//! else writes them and the boot sequence can take their typed operations
//! over. Timer group 1's main watchdog serves the system deadline.

pub use oer_esp32s31_pac::watchdog::WatchdogBlocks;
mod registers;
pub use oer_espressif_watchdog::{
    MainSession, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog, StoppedMainWatchdog,
    SuperWatchdog,
    deadline::{DeadlineBudget, DeadlineError},
};
pub use registers::{Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters};

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

/// Every watchdog of the chip, owned for the image's lifetime.
pub struct Watchdogs {
    /// Timer group 0's main watchdog, only stopped: its watchdog clock is
    /// esp-hal's, which gates it.
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
    pub fn new(blocks: WatchdogBlocks) -> Self {
        let WatchdogBlocks {
            mwdt0,
            mwdt1,
            rwdt,
            swd,
        } = blocks;
        let mwdt0 = Mwdt0Registers::new(mwdt0);
        let mut mwdt1 = Mwdt1Registers::new(mwdt1);
        let rwdt = RwdtRegisters::new(rwdt);
        let swd = SwdRegisters::new(swd);
        mwdt1.start_clock();
        Self {
            main0: StoppedMainWatchdog::new(mwdt0),
            main1: MainWatchdog::new(mwdt1, MAIN_WATCHDOG_TICK),
            rtc: RtcWatchdog::new(rwdt),
            super_watchdog: SuperWatchdog::new(swd),
        }
    }
}

/// The system deadline on timer group 1's main watchdog, measured on `C`.
pub type DeadlineWatchdog<C> =
    oer_espressif_watchdog::deadline::DeadlineWatchdog<Mwdt1Registers, C>;

/// An armed system deadline on timer group 1's main watchdog.
pub type DeadlineLease<C> = oer_espressif_watchdog::deadline::DeadlineLease<Mwdt1Registers, C>;
