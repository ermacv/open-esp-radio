//! Espressif watchdog sequences over a chip's typed watchdog registers.
//!
//! The ESP32-S31 and the ESP32-C5 share the watchdog IP: a main watchdog
//! (MWDT) in each timer group, the RTC watchdog (RWDT) and the super
//! watchdog (SWD) in the low-power watchdog block. Each has a write-protect
//! word; the hardware ignores writes to the watchdog's other words while the
//! word holds anything but [`WRITE_KEY`]. The chip PAC implements the
//! register traits below with its typed field accessors, one field at a
//! time; this crate holds the order of the writes and contains no address.
//!
//! [`Unlocked`] is the write-protect key as a typestate: every write to a
//! watchdog's configuration goes through it, so no write is lost to a
//! locked block, and dropping it stores the lock, so no path leaves the
//! block unlocked. A main watchdog applies its configuration words only
//! when its configuration latch is written; [`Unlocked::commit`] latches and
//! locks. Writing stage actions or timeouts unlatched is not prevented by
//! type, since a feed needs no latch; the sequences here always commit.
//!
//! ```compile_fail,E0599
//! # use oer_espressif_watchdog::{MainWatchdog, MainWatchdogRegisters};
//! fn locked_write<R: MainWatchdogRegisters>(watchdog: &mut MainWatchdog<R>) {
//!     watchdog.feed(); // only an unlocked watchdog is fed
//! }
//! ```
//!
//! ```compile_fail,E0499
//! # use oer_espressif_watchdog::{MainWatchdog, MainWatchdogRegisters};
//! fn two_sessions<R: MainWatchdogRegisters>(watchdog: &mut MainWatchdog<R>) {
//!     let first = watchdog.unlock();
//!     let second = watchdog.unlock();
//!     first.commit();
//! }
//! ```
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod deadline;

#[cfg(test)]
mod tests;

/// The write-protect key that unlocks a watchdog's words; any other value
/// in the write-protect word locks them.
///
/// SOURCE: ESP-IDF 4d59230d `components/esp_hal_wdt/esp32s31/include/hal/mwdt_ll.h:243`
/// and `rwdt_ll.h:25-27` (`RTC_WDT_WKEY_VALUE`, `RTC_WDT_SWD_WKEY_VALUE`),
/// `components/esp_hal_wdt/esp32c5/include/hal/lpwdt_ll.h:27`: one key for
/// the main, RTC and super watchdogs of both chips.
pub const WRITE_KEY: u32 = 0x50D8_3AA1;

/// The value [`Unlocked`] stores to lock a watchdog again.
///
/// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:233` (`mwdt_ll_write_protect_enable`)
/// and `rwdt_ll.h:275-278`.
pub const LOCK: u32 = 0;

/// What a main watchdog stage does when its timeout expires.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32s31/register/soc/timer_group_reg.h:464-470`
/// (`TIMG_WDT_STG0`: 0 no effect, 1 interrupt, 2 CPU reset, 3 system reset).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MainStageAction {
    Off = 0,
    Interrupt = 1,
    ResetCpu = 2,
    ResetSystem = 3,
}

impl MainStageAction {
    /// The stage field's value.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// The length of a reset signal a watchdog drives, as its 3-bit selector.
///
/// SOURCE: ESP-IDF 4d59230d `timer_group_reg.h:388-425`: 0..7 select 8, 16,
/// 24, 32, 40, 64, 128 or 256 watchdog clock cycles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetLength(u8);

impl ResetLength {
    /// The longest reset signal (selector 7), which esp-hal and ESP-IDF
    /// program for both the CPU and the system reset.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:631-676`
    /// (`wdt_cpu_reset_length` and `wdt_sys_reset_length` 7); ESP-IDF
    /// 4d59230d `components/esp_hal_wdt/wdt_hal_iram.c:58-59,79-80` programs
    /// `WDT_RESET_SIG_LENGTH_3_2us`, also 7.
    pub const LONGEST: Self = Self(7);

    /// The selector's value.
    pub const fn code(self) -> u8 {
        self.0
    }
}

/// The main watchdog's configuration word as one value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MainConfig {
    /// The watchdog counts.
    pub enabled: bool,
    /// Flash boot protection, which the ROM arms: a timeout resets the
    /// system even while the watchdog is disabled.
    pub flash_boot: bool,
    /// The actions of stages 0 to 3.
    pub stages: [MainStageAction; 4],
    pub cpu_reset_length: ResetLength,
    pub system_reset_length: ResetLength,
}

impl MainConfig {
    /// Reset the system when stage 0 expires; every other stage off.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:631-676`
    /// (`set_wdt_enabled(true)`).
    pub const SYSTEM_RESET_ON_STAGE0: Self = Self {
        enabled: true,
        flash_boot: false,
        stages: [
            MainStageAction::ResetSystem,
            MainStageAction::Off,
            MainStageAction::Off,
            MainStageAction::Off,
        ],
        cpu_reset_length: ResetLength::LONGEST,
        system_reset_length: ResetLength::LONGEST,
    };
}

/// A watchdog's write-protect word.
pub trait WriteProtected {
    /// Store `key` in the write-protect word.
    fn store_write_key(&mut self, key: u32);
}

/// A main watchdog's words, as the chip PAC's typed fields reach them.
///
/// Each method writes only the fields it names, by read-modify-write, and
/// keeps every other bit of the word.
pub trait MainWatchdogRegisters: WriteProtected {
    /// Write every field of `config` into the configuration word.
    fn configure(&mut self, config: MainConfig);
    /// Write only the enable and flash boot fields of the configuration
    /// word.
    fn set_running(&mut self, enabled: bool, flash_boot: bool);
    /// Write the clock prescaler.
    fn set_prescale(&mut self, prescale: u16);
    /// Reset the prescaler's divider counter (a write trigger).
    fn reset_divider(&mut self);
    /// Write stage 0's timeout, in watchdog ticks.
    fn set_stage0_timeout(&mut self, ticks: u32);
    /// Restart the watchdog from stage 0.
    fn feed(&mut self);
    /// Latch the configuration words into the watchdog's clock domain (a
    /// write trigger).
    fn latch(&mut self);
}

/// The RTC watchdog's words.
pub trait RtcWatchdogRegisters: WriteProtected {
    /// Write only the enable and flash boot fields of the configuration
    /// word.
    fn set_running(&mut self, enabled: bool, flash_boot: bool);
}

/// The super watchdog's words.
pub trait SuperWatchdogRegisters: WriteProtected {
    /// Write only the automatic feed field of the configuration word.
    fn set_auto_feed(&mut self, enabled: bool);
}

/// A watchdog's words while they hold [`WRITE_KEY`]. Dropping it stores
/// [`LOCK`].
#[must_use = "dropping the unlocked watchdog locks it again at once"]
pub struct Unlocked<'a, R: WriteProtected> {
    registers: &'a mut R,
}

impl<'a, R: WriteProtected> Unlocked<'a, R> {
    fn new(registers: &'a mut R) -> Self {
        registers.store_write_key(WRITE_KEY);
        Self { registers }
    }

    /// Lock the watchdog again.
    pub fn lock(self) {}
}

impl<R: WriteProtected> Drop for Unlocked<'_, R> {
    fn drop(&mut self) {
        self.registers.store_write_key(LOCK);
    }
}

impl<R: MainWatchdogRegisters> Unlocked<'_, R> {
    /// Program stage 0's timeout to `timeout`, restarting the divider.
    pub fn set_stage0_timeout(&mut self, tick: MicrosecondTick, timeout: Microseconds) {
        self.registers.set_prescale(tick.prescale());
        self.registers.reset_divider();
        self.registers.set_stage0_timeout(timeout.0);
    }

    /// Write the whole configuration word.
    pub fn configure(&mut self, config: MainConfig) {
        self.registers.configure(config);
    }

    /// Stop counting and clear flash boot protection, keeping the stage
    /// actions and reset lengths.
    ///
    /// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:70-77` (`mwdt_ll_disable`
    /// clears `WDT_EN` only and notes that flash boot protection still
    /// resets) and `components/bootloader_support/src/bootloader_init.c:106-110`
    /// (flash boot protection cleared field by field).
    pub fn stop(&mut self) {
        self.registers.set_running(false, false);
    }

    /// Restart the watchdog from stage 0.
    pub fn feed(&mut self) {
        self.registers.feed();
    }

    /// Latch the configuration written so far.
    pub fn latch(&mut self) {
        self.registers.latch();
    }

    /// Latch the configuration and lock the watchdog.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:655-669`: the
    /// ESP32-S31 latches its configuration asynchronously, a disable
    /// included, and the ESP32-C5 applies stage writes only when latched
    /// (ESP-IDF 4d59230d `components/esp_hal_wdt/esp32c5/include/hal/mwdt_ll.h:122`).
    /// Both chips latch every change here, a disable included; esp-hal
    /// does not latch a disable on the ESP32-C5.
    pub fn commit(mut self) {
        self.latch();
    }
}

impl<R: RtcWatchdogRegisters> Unlocked<'_, R> {
    /// Stop the RTC watchdog and clear its flash boot protection, keeping
    /// the stage actions and reset lengths.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/esp_system/startup_funcs.c:145-153`
    /// (`init_disable_rtc_wdt` clears `WDT_EN` alone, through
    /// `rwdt_ll_disable`) and `bootloader_init.c:86-89` (flash boot
    /// protection cleared alone). esp-hal 1f646c5
    /// `esp-hal/src/rtc_cntl/mod.rs:573-598` stores zero in the whole word,
    /// which also clears the reset lengths and the pause in sleep.
    pub fn stop_rtc(&mut self) {
        self.registers.set_running(false, false);
    }
}

impl<R: SuperWatchdogRegisters> Unlocked<'_, R> {
    /// Let the hardware feed the super watchdog by itself, or stop it
    /// doing so.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/bootloader_support/src/esp32s31/bootloader_esp32s31.c:118-123`
    /// and `esp32c5/bootloader_esp32c5.c:75-80` (`bootloader_super_wdt_auto_feed`
    /// sets `SWD_AUTO_FEED_EN` alone). esp-hal 1f646c5
    /// `esp-hal/src/rtc_cntl/mod.rs:676-686` stores the whole word.
    pub fn set_auto_feed(&mut self, enabled: bool) {
        self.registers.set_auto_feed(enabled);
    }
}

/// The prescaler that makes one main watchdog tick one microsecond of its
/// source clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MicrosecondTick(u16);

impl MicrosecondTick {
    /// The tick of a source clock of `hz`, or `None` unless `hz` is a whole
    /// number of megahertz the 16-bit prescaler holds.
    ///
    /// SOURCE: ESP-IDF 4d59230d `timer_group_reg.h:497-503`: one watchdog
    /// tick is the source clock period times `WDT_CLK_PRESCALE`.
    pub const fn from_source_hz(hz: u32) -> Option<Self> {
        if hz % 1_000_000 != 0 {
            return None;
        }
        let mhz = hz / 1_000_000;
        if mhz == 0 || mhz > u16::MAX as u32 {
            return None;
        }
        Some(Self(mhz as u16))
    }

    /// The prescaler's value: the source clock's megahertz.
    pub const fn prescale(self) -> u16 {
        self.0
    }
}

/// A main watchdog timeout in microseconds, at a [`MicrosecondTick`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Microseconds(u32);

impl Microseconds {
    pub const fn new(micros: core::num::NonZeroU32) -> Self {
        Self(micros.get())
    }
}

/// A main watchdog (MWDT) owned for the image's lifetime.
pub struct MainWatchdog<R: MainWatchdogRegisters> {
    registers: R,
    tick: MicrosecondTick,
}

impl<R: MainWatchdogRegisters> MainWatchdog<R> {
    /// Own the watchdog, whose clock runs at `tick`, and stop it: disabled,
    /// flash boot protection off, latched and locked.
    pub fn new(registers: R, tick: MicrosecondTick) -> Self {
        let mut watchdog = Self { registers, tick };
        let mut unlocked = watchdog.unlock();
        unlocked.stop();
        unlocked.commit();
        watchdog
    }

    /// The tick the watchdog counts.
    pub const fn tick(&self) -> MicrosecondTick {
        self.tick
    }

    /// Unlock the watchdog's words until the returned value is dropped.
    pub fn unlock(&mut self) -> Unlocked<'_, R> {
        Unlocked::new(&mut self.registers)
    }

    /// Reset the system `timeout` from now unless stopped first.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:700-751` then
    /// `:679-687` then `:640-668` (`set_timeout`, `feed`, `enable`): the
    /// timeout is latched before the feed and the enable after it.
    pub fn arm_system_reset(&mut self, timeout: Microseconds) {
        let tick = self.tick;
        let mut unlocked = self.unlock();
        unlocked.set_stage0_timeout(tick, timeout);
        unlocked.latch();
        unlocked.feed();
        unlocked.configure(MainConfig::SYSTEM_RESET_ON_STAGE0);
        unlocked.commit();
    }

    /// Stop the watchdog: disabled, flash boot protection off, latched and
    /// locked.
    pub fn disable(&mut self) {
        let mut unlocked = self.unlock();
        unlocked.stop();
        unlocked.commit();
    }
}

/// The RTC watchdog (RWDT) owned for the image's lifetime.
///
/// The ROM arms it with flash boot protection and the ESP-IDF bootloader
/// arms it again (stage 0 an RTC reset, nine seconds by default) before the
/// image runs; `esp_hal::init` disables it in every image today.
///
/// SOURCE: ESP-IDF 4d59230d `bootloader_init.c:76-104`.
pub struct RtcWatchdog<R: RtcWatchdogRegisters>(R);

impl<R: RtcWatchdogRegisters> RtcWatchdog<R> {
    /// Own the RTC watchdog without touching it.
    pub const fn new(registers: R) -> Self {
        Self(registers)
    }

    /// Unlock the watchdog's words until the returned value is dropped.
    pub fn unlock(&mut self) -> Unlocked<'_, R> {
        Unlocked::new(&mut self.0)
    }

    /// Stop the RTC watchdog and its flash boot protection, field by field.
    pub fn disable(&mut self) {
        self.unlock().stop_rtc();
    }
}

/// The super watchdog (SWD) owned for the image's lifetime.
pub struct SuperWatchdog<R: SuperWatchdogRegisters>(R);

impl<R: SuperWatchdogRegisters> SuperWatchdog<R> {
    /// Own the super watchdog without touching it.
    pub const fn new(registers: R) -> Self {
        Self(registers)
    }

    /// Unlock the watchdog's words until the returned value is dropped.
    pub fn unlock(&mut self) -> Unlocked<'_, R> {
        Unlocked::new(&mut self.0)
    }

    /// Let the hardware feed the super watchdog, as the ESP-IDF bootloader
    /// leaves it.
    pub fn feed_automatically(&mut self) {
        self.unlock().set_auto_feed(true);
    }
}
