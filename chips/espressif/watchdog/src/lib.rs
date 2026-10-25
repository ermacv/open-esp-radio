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
//! The write-protect key is held only inside one operation of this crate:
//! the key is stored before it and the lock after it, however it ends, so no
//! write is lost to a locked block and no path leaves a block unlocked. A
//! main watchdog applies its configuration words only when its
//! configuration latch is written; [`MainWatchdog::with_unlocked`] latches
//! after every operation, before it locks. The register traits take an
//! [`Access`] only this crate makes, so no other code calls them.
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
//!     watchdog.with_unlocked(|_| watchdog.with_unlocked(|_| ()));
//! }
//! ```
//!
//! ```compile_fail,E0521
//! # use oer_espressif_watchdog::{MainSession, MainWatchdog, MainWatchdogRegisters};
//! fn keep_unlocked<'a, R: MainWatchdogRegisters>(
//!     watchdog: &'a mut MainWatchdog<R>,
//!     kept: &mut Option<&'a mut MainSession<'a, R>>,
//! ) {
//!     watchdog.with_unlocked(|session| *kept = Some(session)); // a session never escapes
//! }
//! ```
//!
//! ```compile_fail,E0507
//! # use oer_espressif_watchdog::{MainWatchdog, MainWatchdogRegisters};
//! fn forget_unlocked<R: MainWatchdogRegisters>(watchdog: &mut MainWatchdog<R>) {
//!     watchdog.with_unlocked(|session| core::mem::forget(*session)); // no session is owned
//! }
//! ```
//!
//! ```compile_fail,E0624
//! # use oer_espressif_watchdog::{Access, WRITE_KEY, WriteProtected};
//! fn raw_key<R: WriteProtected>(registers: &mut R) {
//!     registers.store_write_key(Access::new(), WRITE_KEY); // only this crate calls the traits
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

/// The value every operation stores to lock a watchdog again.
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

/// The length of a reset signal a watchdog drives, by its 3-bit selector.
///
/// SOURCE: ESP-IDF 4d59230d `timer_group_reg.h:388-425`: 0..7 select 8, 16,
/// 24, 32, 40, 64, 128 or 256 watchdog clock cycles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetLength {
    /// The longest reset signal (selector 7), which ESP-IDF
    /// program for both the CPU and the system reset.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/esp_hal_wdt/wdt_hal_iram.c:58-59,79-80` programs
    /// `WDT_RESET_SIG_LENGTH_3_2us`, also 7.
    Longest = 7,
}

impl ResetLength {
    /// The selector's value.
    pub const fn code(self) -> u8 {
        self as u8
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
        cpu_reset_length: ResetLength::Longest,
        system_reset_length: ResetLength::Longest,
    };
}

/// Proof that a register call comes from this crate's sequences.
///
/// Every register trait method takes one by value, and only this crate
/// makes one, so the chip PAC implements the traits but no other code calls
/// them: the write-protect key, the configuration words and the latch are
/// reached only through the sessions below.
pub struct Access<'a>(core::marker::PhantomData<&'a mut ()>);

impl Access<'_> {
    const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

/// A watchdog's write-protect word.
pub trait WriteProtected {
    /// Store `key` in the write-protect word.
    fn store_write_key(&mut self, access: Access<'_>, key: u32);
}

/// The fields that stop a main watchdog: enable, flash boot and the
/// configuration latch. A main watchdog whose clock the image does not run
/// implements only these.
///
/// Each method writes only the fields it names, by read-modify-write, and
/// keeps every other bit of the word.
pub trait MainWatchdogEnable: WriteProtected {
    /// Write only the enable and flash boot fields of the configuration
    /// word.
    fn set_running(&mut self, access: Access<'_>, enabled: bool, flash_boot: bool);
    /// Latch the configuration words into the watchdog's clock domain (a
    /// write trigger).
    fn latch(&mut self, access: Access<'_>);
}

/// A main watchdog's words, as the chip PAC's typed fields reach them.
///
/// Each method writes only the fields it names, by read-modify-write, and
/// keeps every other bit of the word.
pub trait MainWatchdogRegisters: MainWatchdogEnable {
    /// Write every field of `config` into the configuration word.
    fn configure(&mut self, access: Access<'_>, config: MainConfig);
    /// Write the clock prescaler.
    fn set_prescale(&mut self, access: Access<'_>, prescale: u16);
    /// Reset the prescaler's divider counter (a write trigger).
    fn reset_divider(&mut self, access: Access<'_>);
    /// Write stage 0's timeout, in watchdog ticks.
    fn set_stage0_timeout(&mut self, access: Access<'_>, ticks: u32);
    /// Restart the watchdog from stage 0.
    fn feed(&mut self, access: Access<'_>);
}

/// The RTC watchdog's words.
pub trait RtcWatchdogRegisters: WriteProtected {
    /// Write only the enable and flash boot fields of the configuration
    /// word.
    fn set_running(&mut self, access: Access<'_>, enabled: bool, flash_boot: bool);
}

/// The super watchdog's words.
pub trait SuperWatchdogRegisters: WriteProtected {
    /// Write only the automatic feed field of the configuration word.
    fn set_auto_feed(&mut self, access: Access<'_>, enabled: bool);
}

/// A watchdog's words while they hold [`WRITE_KEY`]; dropping it stores
/// [`LOCK`]. It lives only inside one of this crate's operations, so no
/// caller holds, forgets or leaks it.
struct Unlocked<'a, R: WriteProtected> {
    registers: &'a mut R,
}

impl<'a, R: WriteProtected> Unlocked<'a, R> {
    fn new(registers: &'a mut R) -> Self {
        registers.store_write_key(Access::new(), WRITE_KEY);
        Self { registers }
    }
}

impl<R: WriteProtected> Drop for Unlocked<'_, R> {
    fn drop(&mut self) {
        self.registers.store_write_key(Access::new(), LOCK);
    }
}

/// An unlocked main watchdog that latches its configuration when dropped,
/// before [`Unlocked`] locks it.
struct Latched<'a, R: MainWatchdogEnable>(Unlocked<'a, R>);

impl<R: MainWatchdogEnable> Latched<'_, R> {
    fn registers(&mut self) -> &mut R {
        self.0.registers
    }
}

impl<R: MainWatchdogEnable> Drop for Latched<'_, R> {
    fn drop(&mut self) {
        self.registers().latch(Access::new());
    }
}

/// A main watchdog's words inside [`MainWatchdog::with_unlocked`].
///
/// The operation stores [`WRITE_KEY`] before it and, however it ends, a
/// panic included, latches the configuration and stores [`LOCK`] after it,
/// so no change is left unlatched and no path leaves the watchdog unlocked.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:655-669`: the
/// ESP32-S31 latches its configuration asynchronously, a disable included,
/// and the ESP32-C5 applies stage writes only when latched (ESP-IDF 4d59230d
/// `components/esp_hal_wdt/esp32c5/include/hal/mwdt_ll.h:122`). Both chips
/// latch every session here, a disable included; esp-hal does not latch a
/// disable on the ESP32-C5.
pub struct MainSession<'a, R: MainWatchdogRegisters> {
    latched: Latched<'a, R>,
    tick: MicrosecondTick,
}

impl<R: MainWatchdogRegisters> MainSession<'_, R> {
    /// Program stage 0's timeout to `timeout` at the watchdog's tick,
    /// restarting the divider.
    pub fn set_stage0_timeout(&mut self, timeout: Microseconds) {
        let prescale = self.tick.prescale();
        let registers = self.latched.registers();
        registers.set_prescale(Access::new(), prescale);
        registers.reset_divider(Access::new());
        registers.set_stage0_timeout(Access::new(), timeout.0);
    }

    /// Write the whole configuration word.
    pub fn configure(&mut self, config: MainConfig) {
        self.latched.registers().configure(Access::new(), config);
    }

    /// Stop counting and clear flash boot protection, keeping the stage
    /// actions and reset lengths.
    ///
    /// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:70-77` (`mwdt_ll_disable`
    /// clears `WDT_EN` only and notes that flash boot protection still
    /// resets) and `components/bootloader_support/src/bootloader_init.c:106-110`
    /// (flash boot protection cleared field by field).
    pub fn stop(&mut self) {
        self.latched
            .registers()
            .set_running(Access::new(), false, false);
    }

    /// Restart the watchdog from stage 0.
    pub fn feed(&mut self) {
        self.latched.registers().feed(Access::new());
    }

    /// Latch the configuration written so far; the session latches again
    /// when it ends.
    pub fn latch(&mut self) {
        self.latched.registers().latch(Access::new());
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
        if !hz.is_multiple_of(1_000_000) {
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
        watchdog.disable();
        watchdog
    }

    /// The tick the watchdog counts.
    pub const fn tick(&self) -> MicrosecondTick {
        self.tick
    }

    /// Run `operation` on the unlocked watchdog: store [`WRITE_KEY`], run
    /// it, then latch the configuration and store [`LOCK`], however
    /// `operation` ends.
    pub fn with_unlocked<T>(&mut self, operation: impl FnOnce(&mut MainSession<'_, R>) -> T) -> T {
        let mut session = MainSession {
            latched: Latched(Unlocked::new(&mut self.registers)),
            tick: self.tick,
        };
        operation(&mut session)
    }

    /// Reset the system `timeout` from now unless stopped first.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/timer/timg.rs:700-751` then
    /// `:679-687` then `:640-668` (`set_timeout`, `feed`, `enable`): the
    /// timeout is latched before the feed and the enable after it.
    pub fn arm_system_reset(&mut self, timeout: Microseconds) {
        self.with_unlocked(|session| {
            session.set_stage0_timeout(timeout);
            session.latch();
            session.feed();
            session.configure(MainConfig::SYSTEM_RESET_ON_STAGE0);
        });
    }

    /// Stop the watchdog: disabled, flash boot protection off, latched and
    /// locked.
    pub fn disable(&mut self) {
        self.with_unlocked(|session| session.stop());
    }
}

/// A main watchdog whose clock the image does not run, owned so that
/// nothing else writes it. It is only stopped: armed without its clock it
/// would never count.
pub struct StoppedMainWatchdog<R: MainWatchdogEnable>(R);

impl<R: MainWatchdogEnable> StoppedMainWatchdog<R> {
    /// Own the watchdog and stop it: disabled, flash boot protection off,
    /// latched and locked.
    pub fn new(registers: R) -> Self {
        let mut watchdog = Self(registers);
        watchdog.disable();
        watchdog
    }

    /// Stop the watchdog again: disabled, flash boot protection off,
    /// latched and locked.
    ///
    /// SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:70-77` and
    /// `bootloader_init.c:106-110`, as [`MainSession::stop`].
    pub fn disable(&mut self) {
        let mut latched = Latched(Unlocked::new(&mut self.0));
        latched.registers().set_running(Access::new(), false, false);
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

    /// Stop the RTC watchdog and clear its flash boot protection, keeping
    /// the stage actions and reset lengths: unlocked, the two fields
    /// written, locked.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/esp_system/startup_funcs.c:145-153`
    /// (`init_disable_rtc_wdt` clears `WDT_EN` alone, through
    /// `rwdt_ll_disable`) and `bootloader_init.c:86-89` (flash boot
    /// protection cleared alone). esp-hal 1f646c5
    /// `esp-hal/src/rtc_cntl/mod.rs:573-598` stores zero in the whole word,
    /// which also clears the reset lengths and the pause in sleep.
    pub fn disable(&mut self) {
        let unlocked = Unlocked::new(&mut self.0);
        unlocked.registers.set_running(Access::new(), false, false);
    }
}

/// The super watchdog (SWD) owned for the image's lifetime.
pub struct SuperWatchdog<R: SuperWatchdogRegisters>(R);

impl<R: SuperWatchdogRegisters> SuperWatchdog<R> {
    /// Own the super watchdog without touching it.
    pub const fn new(registers: R) -> Self {
        Self(registers)
    }

    /// Let the hardware feed the super watchdog, as the ESP-IDF bootloader
    /// leaves it: unlocked, the automatic feed field set, locked.
    ///
    /// SOURCE: ESP-IDF 4d59230d `components/bootloader_support/src/esp32s31/bootloader_esp32s31.c:118-123`
    /// and `esp32c5/bootloader_esp32c5.c:75-80` (`bootloader_super_wdt_auto_feed`
    /// sets `SWD_AUTO_FEED_EN` alone). esp-hal 1f646c5
    /// `esp-hal/src/rtc_cntl/mod.rs:676-686` stores the whole word.
    pub fn feed_automatically(&mut self) {
        let unlocked = Unlocked::new(&mut self.0);
        unlocked.registers.set_auto_feed(Access::new(), true);
    }
}
