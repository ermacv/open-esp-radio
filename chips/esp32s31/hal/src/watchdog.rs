//! The chip's watchdogs: both timer groups' main watchdogs, the RTC
//! watchdog and the super watchdog, owned from the reset root's
//! `WatchdogPeripherals`.
//!
//! The bootstrap takes the RTC, super and timer group 0's main watchdogs
//! over from the bootloader (`oer_espressif_boot::watchdog`) and disables
//! the RTC watchdog right before its jump; stage two owns all four
//! ([`Watchdogs`]) and arms the RTC watchdog again as its backstop the
//! moment it takes them: stage 0 resets the chip
//! [`RTC_WATCHDOG_BUDGET_MS`] after the last feed. The image's executor
//! feeds it from a task ([`ArmedRtcWatchdog::feed_every`] every
//! [`RTC_WATCHDOG_FEED_PERIOD_MS`]), and a fatal path that cannot prepare
//! the chip's reset (`cores::AwaitWatchdog`) waits for it. Timer group 1's
//! main watchdog serves the system deadline.

mod registers;
pub use oer_espressif_watchdog::{
    ArmedRtcWatchdog, MainSession, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog,
    StoppedMainWatchdog, SuperWatchdog,
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

/// Stage two's RTC watchdog budget: the reset follows this long after the
/// last feed. It is the ESP-IDF bootloader's default backstop, the one the
/// bootstrap's boot waits run under.
///
/// SOURCE: ESP-IDF 4d59230d `components/bootloader/Kconfig.projbuild:311-315`
/// (`CONFIG_BOOTLOADER_WDT_TIME_MS`, 9000 by default).
pub const RTC_WATCHDOG_BUDGET_MS: u32 = 9_000;

/// How often the executor's task feeds the RTC watchdog: well inside its
/// budget, so a late poll of a few seconds still feeds in time.
pub const RTC_WATCHDOG_FEED_PERIOD_MS: u32 = 1_000;

/// The RTC slow clock the RTC watchdog counts, at reset the internal RC
/// oscillator.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32s31/include/soc/clk_tree_defs.h:52,101-104`
/// (`SOC_CLK_RC_SLOW_FREQ_APPROX` 136 kHz, the default `RTC_SLOW_CLK`
/// source).
const RTC_SLOW_CLOCK_HZ: u32 = 136_000;

/// Stage 0's hold word for [`RTC_WATCHDOG_BUDGET_MS`]: the budget's slow
/// clock ticks shifted right by one. The hardware counts the word shifted
/// back by one plus the eFuse `WDT_DELAY_SEL` bit, so the reset follows the
/// budget, or twice it on a chip whose eFuse selects the longer delay.
///
/// SOURCE: ESP-IDF 4d59230d `rwdt_ll.h:117-123` (`config1 = timeout_ticks
/// >> (1 + EFUSE_WDT_DELAY_SEL)`) and `efuse_reg.h:650-659`
/// (`EFUSE_WDT_DELAY_SEL`, one bit).
pub const RTC_WATCHDOG_STAGE0_HOLD: u32 =
    ((RTC_WATCHDOG_BUDGET_MS as u64 * RTC_SLOW_CLOCK_HZ as u64 / 1_000) >> 1) as u32;

/// The watchdogs' register owners, split from the root's partition.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct WatchdogRegisters {
    /// Timer group 0's main watchdog and its clock and reset word.
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
            hp_sys_clkrst_timg0,
            hp_sys_clkrst_timg1,
            lp_wdt,
            lp_wdt_swd,
        } = peripherals;
        Self {
            main0: Mwdt0Registers::new(timg0_wdt, hp_sys_clkrst_timg0),
            main1: Mwdt1Registers::new(timg1_wdt, hp_sys_clkrst_timg1),
            rtc: RwdtRegisters::new(lp_wdt),
            super_watchdog: SwdRegisters::new(lp_wdt_swd),
        }
    }
}

/// Every watchdog of the chip, owned for stage two's lifetime.
pub struct Watchdogs {
    /// Timer group 0's main watchdog, only stopped: its watchdog clock is
    /// not the image's, which gates it. It retains the group clock/reset
    /// word after selecting and enabling T0's XTAL clock at startup.
    pub main0: StoppedMainWatchdog<Mwdt0Registers>,
    /// Timer group 1's main watchdog, the system deadline's timer.
    pub main1: MainWatchdog<Mwdt1Registers>,
    /// The RTC watchdog, armed: the image's executor feeds it.
    pub rtc: ArmedRtcWatchdog<RwdtRegisters>,
    /// The super watchdog.
    pub super_watchdog: SuperWatchdog<SwdRegisters>,
}

impl Watchdogs {
    /// Own every watchdog: run timer group 1 from reset on the XTAL clock,
    /// stop both main watchdogs, latched and locked, and arm the RTC
    /// watchdog for [`RTC_WATCHDOG_BUDGET_MS`]. Enable T0's XTAL clock
    /// without resetting its group or changing the watchdog clock.
    /// The super watchdog is left as the bootstrap leaves it, fed by hardware.
    pub fn new(registers: WatchdogRegisters) -> Self {
        let WatchdogRegisters {
            mut main0,
            mut main1,
            rtc,
            super_watchdog,
        } = registers;
        main0.start_timer0_clock();
        main1.start_clock();
        Self {
            main0: StoppedMainWatchdog::new(main0),
            main1: MainWatchdog::new(main1, MAIN_WATCHDOG_TICK),
            rtc: RtcWatchdog::new(rtc).arm_rtc_reset(RTC_WATCHDOG_STAGE0_HOLD),
            super_watchdog: SuperWatchdog::new(super_watchdog),
        }
    }
}

/// The system deadline on timer group 1's main watchdog, measured on `C`.
pub type DeadlineWatchdog<C> =
    oer_espressif_watchdog::deadline::DeadlineWatchdog<Mwdt1Registers, C>;

/// An armed system deadline on timer group 1's main watchdog.
pub type DeadlineLease<C> = oer_espressif_watchdog::deadline::DeadlineLease<Mwdt1Registers, C>;
