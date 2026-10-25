//! The per-domain transfers of platform blocks from esp-hal's singletons to
//! the chip PAC: the eFuse MAC words, the true random number generator, the
//! reset causes, the watchdogs, the interrupt matrix with the CLIC and the
//! USB Serial/JTAG console.
//!
//! Each function consumes the domain's esp-hal singletons, which esp-hal
//! hands out once, and never returns them, so it runs at most once per
//! program and no esp-hal owner of the blocks remains. esp-hal hands out no
//! `EFUSE` singleton, so the eFuse transfer succeeds once per program by a
//! flag instead. No function makes an MMIO access.

use esp_hal::peripherals::{
    CLIC, INTERRUPT_CORE0, INTERRUPT_CORE1, LP_AON_CLK_RST, LP_PERI, LP_WDT, RNG, TIMG0, TIMG1,
    USB_DEVICE,
};
use oer_esp32s31_hal::platform::InterruptBlocks;
use oer_esp32s31_hal::usb_serial_jtag::{
    RetireError, UsbSerialJtagHandoff, UsbSerialJtagRegisters, retire_early_writer,
};
use oer_esp32s31_pac::platform::{
    EfuseHandoff, EfuseRegisters, EntropyHandoff, EntropyRegisters, InterruptMatrixHandoff,
    InterruptMatrixPeripherals, ResetCauseHandoff, ResetCauseRegisters,
};
use oer_esp32s31_pac::watchdog::{WatchdogBlocks, WatchdogHandoff};

/// Transfer the interrupt matrix and the CLIC: hart 0's controller, which
/// the interrupt table's router installs, the pending sources, and hart 1's
/// bank.
///
/// Call it after `esp_hal::init`, whose `setup_interrupts` is esp-hal's last
/// write of the route and line words.
pub fn take_interrupt_controller(
    interrupt_core0: INTERRUPT_CORE0<'static>,
    interrupt_core1: INTERRUPT_CORE1<'static>,
    clic: CLIC<'static>,
) -> InterruptBlocks {
    let _consumed = (interrupt_core0, interrupt_core1, clic);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent interrupt-matrix handoff"
    )]
    // SAFETY: esp-hal issues `INTERRUPT_CORE0`, `INTERRUPT_CORE1` and `CLIC`
    // once; all three are consumed above and never returned, so this runs at
    // most once. esp-hal writes the route and CLIC words only in
    // `esp_hal::init`, which has returned, and in its interrupt API
    // (`interrupt::enable`, `map`, `bind_interrupt` and the drivers'
    // interrupt handlers), which no image calls; the words are reviewed in
    // `registers/shared-words.toml`.
    let handoff = unsafe { InterruptMatrixHandoff::assume_exclusive() };
    InterruptBlocks::new(InterruptMatrixPeripherals::from_handoff(handoff))
}

/// Whether the eFuse words were transferred.
static EFUSE_TAKEN: crate::once::Once = crate::once::Once::new();

/// Transfer the eFuse read words; `None` after the first transfer.
pub fn take_efuse() -> Option<EfuseRegisters> {
    if !EFUSE_TAKEN.first() {
        return None;
    }
    #[allow(
        unsafe_code,
        reason = "the once flag proves the permanent eFuse handoff"
    )]
    // SAFETY: the flag above lets this run once per program. esp-hal keeps
    // `EFUSE` hidden, hands out no owner of it and never writes the read
    // words; its readers (`esp_hal::efuse`, the chip revision in
    // `esp_hal::init`) only load them, which the owner's reads do not
    // disturb.
    let handoff = unsafe { EfuseHandoff::assume_exclusive() };
    Some(EfuseRegisters::from_handoff(handoff))
}

/// Transfer the true random number generator and its clock and reset word.
///
/// `esp_hal::init` enabled and configured the generator; its owner only
/// reads it.
pub fn take_entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>) -> EntropyRegisters {
    let _consumed = (rng, lp_peri);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent entropy handoff"
    )]
    // SAFETY: esp-hal issues `RNG` and `LP_PERI` once; both are consumed
    // above and never returned, so this runs at most once. esp-hal's later
    // writers of the TRNG words (`TrngSource::new`) and readers (`Rng`,
    // `Trng`) are not called by any image; `TrngSource::new` needs `RNG`.
    let handoff = unsafe { EntropyHandoff::assume_exclusive() };
    EntropyRegisters::from_handoff(handoff)
}

/// Transfer the reset-cause words of both HP cores.
pub fn take_reset_causes(lp_aon_clk_rst: LP_AON_CLK_RST<'static>) -> ResetCauseRegisters {
    let _consumed = lp_aon_clk_rst;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owner proves the permanent reset-cause handoff"
    )]
    // SAFETY: esp-hal issues `LP_AON_CLK_RST` once; it is consumed above and
    // never returned, so this runs at most once. esp-hal's only reader of the
    // cause words is the ROM getter behind `reset_reason`, which no image
    // calls, and esp-hal does not write them.
    let handoff = unsafe { ResetCauseHandoff::assume_exclusive() };
    ResetCauseRegisters::from_handoff(handoff)
}

/// Transfer both timer groups' main watchdogs, timer group 1's clock and
/// reset word, and the RTC and super watchdogs.
///
/// `esp_hal::init` stopped every watchdog; the HAL's `Watchdogs` owns them
/// from here.
pub fn take_watchdogs(
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
    lp_wdt: LP_WDT<'static>,
) -> WatchdogBlocks {
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
    WatchdogBlocks::from_handoff(handoff)
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
/// An early record holds the block now, or the fatal path took it.
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
    // early record holds the block, and no record takes it afterwards. The
    // fatal path takes only a free block and never one the driver holds.
    let handoff = unsafe { UsbSerialJtagHandoff::assume_exclusive() };
    Ok(UsbSerialJtagRegisters::from_handoff(handoff))
}
