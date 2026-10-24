//! The per-domain transfers of platform blocks from esp-hal's singletons to
//! the chip PAC: the random number generator, the reset cause, the
//! watchdogs and the USB Serial/JTAG console.
//!
//! Each function consumes the domain's esp-hal singletons, which esp-hal
//! hands out once, and never returns them, so it runs at most once per
//! program and no esp-hal owner of the blocks remains. No function makes
//! an MMIO access.

use esp_hal::{
    clock::xtal_clock,
    peripherals::{LP_CLKRST, LP_PERI, LP_WDT, RNG, TIMG0, TIMG1, USB_DEVICE},
};
use oer_esp32c5_hal::platform::{
    EntropyHandoff, EntropyRegisters, ResetCauseHandoff, ResetCauseRegisters,
};
use oer_esp32c5_hal::usb_serial_jtag::{
    RetireError, UsbSerialJtagHandoff, UsbSerialJtagRegisters, retire_early_writer,
};
use oer_esp32c5_hal::watchdog::{MicrosecondTick, WatchdogBlocks, WatchdogHandoff, Watchdogs};

/// Transfer the random number generator's clock gate, configuration and
/// output.
///
/// esp-hal does not enable the ESP32-C5 generator; the HAL's entropy owner
/// enables it.
pub fn take_entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>) -> EntropyRegisters {
    let _consumed = (rng, lp_peri);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent entropy handoff"
    )]
    // SAFETY: esp-hal issues `RNG` and `LP_PERI` (both the `LPPERI` block)
    // once; both are consumed above and never returned, so this runs at
    // most once. esp-hal writes no `RNG_CFG`; its other writers of
    // `CLK_EN` are reviewed in `registers/shared-words.toml`, and its `Rng`
    // reads the separate `RNG_DATA` word.
    let handoff = unsafe { EntropyHandoff::assume_exclusive() };
    EntropyRegisters::from_handoff(handoff)
}

/// Transfer the HP core's reset-cause word.
pub fn take_reset_cause(lp_clkrst: LP_CLKRST<'static>) -> ResetCauseRegisters {
    let _consumed = lp_clkrst;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owner proves the permanent reset-cause handoff"
    )]
    // SAFETY: esp-hal issues `LP_CLKRST` once; it is consumed above and
    // never returned, so this runs at most once. esp-hal's only reader of
    // the cause word is the ROM getter behind `reset_reason`, which no image
    // calls, and esp-hal does not write it.
    let handoff = unsafe { ResetCauseHandoff::assume_exclusive() };
    ResetCauseRegisters::from_handoff(handoff)
}

/// Transfer the watchdogs once and own them, the main watchdogs counting
/// microseconds of the XTAL clock esp-hal detected.
///
/// `esp_hal::init` stopped every watchdog; [`Watchdogs::new`] stops both
/// main watchdogs again, latched.
pub fn take_watchdogs(
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
    lp_wdt: LP_WDT<'static>,
) -> Watchdogs {
    let _consumed = (timg0, timg1, lp_wdt);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent watchdog handoff"
    )]
    // SAFETY: esp-hal issues `TIMG0`, `TIMG1` and `LP_WDT` once; all three
    // are consumed above and never returned, so this runs at most once.
    // esp-hal writes the watchdog words only in `esp_hal::init`, which has
    // returned, and in its `TimerGroup`, `Wdt` and `Rtc` drivers: `TimerGroup`
    // needs a consumed timer group, `Rtc` is built only inside `init`, and
    // no image calls `Wdt::new` or `Wdt::set_wdt_enabled` on its own. The
    // RTC calibration words of timer group 0 that esp-hal's clock code
    // writes are not part of the watchdog blocks.
    let handoff = unsafe { WatchdogHandoff::assume_exclusive() };
    // SOURCE: esp-hal 1f646c5 `esp-hal/src/clock/mod.rs:571-574`
    // (`xtal_clock`, the 40 or 48 MHz XTAL `esp_hal::init` detected).
    #[expect(
        clippy::expect_used,
        reason = "esp-hal detects a 40 or 48 MHz XTAL at init, both whole megahertz"
    )]
    let tick =
        MicrosecondTick::from_source_hz(xtal_clock().as_hz()).expect("the C5 XTAL is 40 or 48 MHz");
    Watchdogs::new(WatchdogBlocks::from_handoff(handoff), tick)
}

/// The console's driver could not take the USB Serial/JTAG block: the
/// singleton comes back, for another try once an early record ended.
pub struct UsbSerialJtagHeld {
    pub usb_device: USB_DEVICE<'static>,
    pub reason: RetireError,
}

impl core::fmt::Debug for UsbSerialJtagHeld {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UsbSerialJtagHeld")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Transfer the USB Serial/JTAG console's blocks from the early writer and
/// esp-hal to the console's driver, if no early record holds them. Never
/// waits.
///
/// The early writer is retired first: no early record takes the block
/// afterwards, and the driver's owner reports what the early writer dropped
/// (`take_early_lost`).
///
/// # Errors
/// An early record holds the block now, or the driver took it before.
pub fn take_usb_serial_jtag(
    usb_device: USB_DEVICE<'static>,
) -> Result<UsbSerialJtagRegisters, UsbSerialJtagHeld> {
    let _retired = match retire_early_writer() {
        Ok(retired) => retired,
        Err(reason) => return Err(UsbSerialJtagHeld { usb_device, reason }),
    };
    let _consumed = usb_device;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owner and the retired early writer prove the console handoff"
    )]
    // SAFETY: esp-hal issues `USB_DEVICE` once; it is consumed above and
    // never returned, so this runs at most once and no esp-hal driver of the
    // block exists. esp-hal's console and its `esp_hal::init` never write
    // the block without that singleton. The early writer, the block's only
    // other owner, gave it up above: the retirement succeeds only while no
    // early record holds the block, and no record takes it afterwards.
    let handoff = unsafe { UsbSerialJtagHandoff::assume_exclusive() };
    Ok(UsbSerialJtagRegisters::from_handoff(handoff))
}
