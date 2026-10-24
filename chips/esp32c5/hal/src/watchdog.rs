//! The chip's watchdogs: both timer groups' main watchdogs, the RTC
//! watchdog and the super watchdog, owned from their platform handoff.
//!
//! `esp_hal::init` still stops the RTC and super watchdogs and timer group
//! 0's main watchdog in every image; [`Watchdogs`] owns them so nothing
//! else writes them and the boot sequence can take their typed operations
//! over. Timer group 1's main watchdog serves the system deadline.

pub use oer_esp32c5_pac::watchdog::{
    Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters, WatchdogBlocks, WatchdogHandoff,
};
pub use oer_espressif_watchdog::{
    MainSession, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog, StoppedMainWatchdog,
    SuperWatchdog,
    deadline::{DeadlineBudget, DeadlineError},
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
    /// Own every watchdog: run timer group 1 from reset on the XTAL clock,
    /// which its main watchdog counts at `tick`, and stop both main
    /// watchdogs, latched and locked. The RTC and super watchdogs are left
    /// as they are. The XTAL is 40 or 48 MHz, a fact of the module that
    /// esp-hal detects.
    pub fn new(blocks: WatchdogBlocks, tick: MicrosecondTick) -> Self {
        let WatchdogBlocks {
            mwdt0,
            mut mwdt1,
            rwdt,
            swd,
        } = blocks;
        mwdt1.start_clock();
        Self {
            main0: StoppedMainWatchdog::new(mwdt0),
            main1: MainWatchdog::new(mwdt1, tick),
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
