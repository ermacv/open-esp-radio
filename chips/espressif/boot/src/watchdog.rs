//! The bootstrap's watchdogs.
//!
//! The ROM arms the RTC watchdog and timer group 0's main watchdog with
//! flash boot protection; the ESP-IDF bootloader clears both protections
//! and arms the RTC watchdog again (stage 0 an RTC reset, nine seconds by
//! default) before the image runs. The bootstrap keeps that RTC watchdog
//! armed as its backstop: every boot wait runs under one deadline below it
//! ([`crate::wait::BootBudget`]). [`BootWatchdogs::take_over`] stops timer
//! group 0's main watchdog (its flash boot protection cleared) and lets the
//! hardware feed the super watchdog, as the bootloader leaves it;
//! [`BootWatchdogs::hand_off`] disables the RTC watchdog last, right before
//! the jump to stage two, which adopts the watchdogs as they are.
//!
//! Timer group 1's main watchdog is not armed by the ROM and its bus clock
//! is gated at reset, so the bootstrap leaves it alone.
//!
//! SOURCE: ESP-IDF 4d59230d
//! `components/bootloader_support/src/bootloader_init.c:76-110`
//! (`bootloader_config_wdt`: RWDT flash boot protection cleared, the RWDT
//! armed for `CONFIG_BOOTLOADER_WDT_TIME_MS`, then MWDT0's protection
//! cleared), `components/bootloader/Kconfig.projbuild:311-315` (9000 ms by
//! default), `components/esp_system/startup_funcs.c:145-153` (the
//! application disables the RWDT once it runs) and
//! `components/bootloader_support/src/esp32s31/bootloader_esp32s31.c:118-123`
//! (the super watchdog fed by hardware).

use oer_espressif_watchdog::{
    MainWatchdogEnable, RtcWatchdog, RtcWatchdogRegisters, StoppedMainWatchdog, SuperWatchdog,
    SuperWatchdogRegisters,
};

/// The bootstrap's watchdogs while it brings the chip up: the RTC watchdog
/// still armed.
#[must_use = "the RTC watchdog stays armed until the boot hands off"]
pub struct BootWatchdogs<M, R: RtcWatchdogRegisters, S: SuperWatchdogRegisters>
where
    M: MainWatchdogEnable,
{
    main0: StoppedMainWatchdog<M>,
    rtc: RtcWatchdog<R>,
    super_watchdog: SuperWatchdog<S>,
}

impl<M, R, S> BootWatchdogs<M, R, S>
where
    M: MainWatchdogEnable,
    R: RtcWatchdogRegisters,
    S: SuperWatchdogRegisters,
{
    /// Own the watchdogs at the bootstrap's start: the super watchdog fed
    /// by hardware, timer group 0's main watchdog stopped, latched and
    /// locked, the RTC watchdog left armed.
    pub fn take_over(main0: M, rtc: R, super_watchdog: S) -> Self {
        let mut super_watchdog = SuperWatchdog::new(super_watchdog);
        super_watchdog.feed_automatically();
        Self {
            main0: StoppedMainWatchdog::new(main0),
            rtc: RtcWatchdog::new(rtc),
            super_watchdog,
        }
    }

    /// Disable the RTC watchdog, the boot's last write before the jump to
    /// stage two, and return the watchdogs as stage two adopts them.
    pub fn hand_off(self) -> HandedOff<M, R, S> {
        let Self {
            main0,
            mut rtc,
            super_watchdog,
        } = self;
        rtc.disable();
        HandedOff {
            main0,
            rtc,
            super_watchdog,
        }
    }
}

/// The watchdogs as the bootstrap leaves them: both stopped, the super
/// watchdog fed by hardware.
#[must_use = "the handed-off watchdogs end with the bootstrap"]
pub struct HandedOff<M, R: RtcWatchdogRegisters, S: SuperWatchdogRegisters>
where
    M: MainWatchdogEnable,
{
    pub main0: StoppedMainWatchdog<M>,
    pub rtc: RtcWatchdog<R>,
    pub super_watchdog: SuperWatchdog<S>,
}
