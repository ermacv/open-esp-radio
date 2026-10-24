//! Why the HP core last left reset: the codes of the reset-cause field.
//!
//! The core's read-only cause word is `LP_CLKRST.RESET_CAUSE`; its five-bit
//! cause field (bits 4:0) keeps the code until the next reset.
//!
//! SOURCE: ESP-IDF 4d59230d
//! `components/soc/esp32c5/register/soc/lp_clkrst_reg.h`
//! (`LP_CLKRST_RESET_CAUSE_REG` at `DR_REG_LP_CLKRST_BASE` + 0x10).

/// A reset-cause code, as the cause field holds it.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/soc/esp32c5/include/soc/reset_reasons.h:29-50`
/// (`soc_reset_reason_t`). The levels in the names follow its terminology
/// (lines 9-19): a CPU reset resets one core only; a core reset the digital
/// system except the RTC sub-system; a system reset the digital system with
/// the RTC sub-system; a chip reset the whole chip with its analog part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// 0x01 `CHIP_POWER_ON` and `CHIP_BROWN_OUT`, which share the code:
    /// power-on reset, or an unstable VDD reset the chip.
    ChipPowerOn,
    /// 0x03 `CORE_SW`: software reset of the digital core.
    CoreSoftware,
    /// 0x05 `CORE_DEEP_SLEEP`: deep sleep reset the digital core.
    CoreDeepSleep,
    /// 0x07 `CORE_MWDT0`: main watchdog 0 reset the digital core.
    CoreMainWatchdog0,
    /// 0x08 `CORE_MWDT1`: main watchdog 1 reset the digital core.
    CoreMainWatchdog1,
    /// 0x09 `CORE_RTC_WDT`: the RTC watchdog reset the digital core.
    CoreRtcWatchdog,
    /// 0x0b `CPU0_MWDT0`: main watchdog 0 reset CPU 0.
    CpuMainWatchdog0,
    /// 0x0c `CPU0_SW`: software reset of CPU 0.
    CpuSoftware,
    /// 0x0d `CPU0_RTC_WDT`: the RTC watchdog reset CPU 0.
    CpuRtcWatchdog,
    /// 0x0f `SYS_BROWN_OUT`: VDD was not stable and reset the digital core.
    SystemBrownOut,
    /// 0x10 `SYS_RTC_WDT`: the RTC watchdog reset the digital core and the
    /// RTC module.
    SystemRtcWatchdog,
    /// 0x11 `CPU0_MWDT1`: main watchdog 1 reset CPU 0.
    CpuMainWatchdog1,
    /// 0x12 `SYS_SUPER_WDT`: the super watchdog reset the digital core and
    /// the RTC module.
    SystemSuperWatchdog,
    /// 0x14 `CORE_EFUSE_CRC`: an eFuse CRC error reset the digital core.
    CoreEfuseCrc,
    /// 0x15 `CORE_USB_UART`: the USB Serial/JTAG controller's UART reset the
    /// digital core.
    CoreUsbUart,
    /// 0x16 `CORE_USB_JTAG`: the USB Serial/JTAG controller's JTAG reset the
    /// digital core.
    CoreUsbJtag,
    /// 0x18 `CPU0_JTAG`: JTAG reset CPU 0.
    CpuJtag,
    /// 0x19 `CORE_PWR_GLITCH`: a power glitch reset the digital core and the
    /// RTC module.
    CorePowerGlitch,
    /// 0x1a `CPU0_LOCKUP`: the CPU locked up (an exception inside the
    /// exception handler).
    CpuLockup,
    /// A code the table does not name, as read.
    Other(u8),
}

impl ResetCause {
    /// The cause a code names; an unnamed code is [`Self::Other`].
    pub const fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::ChipPowerOn,
            0x03 => Self::CoreSoftware,
            0x05 => Self::CoreDeepSleep,
            0x07 => Self::CoreMainWatchdog0,
            0x08 => Self::CoreMainWatchdog1,
            0x09 => Self::CoreRtcWatchdog,
            0x0b => Self::CpuMainWatchdog0,
            0x0c => Self::CpuSoftware,
            0x0d => Self::CpuRtcWatchdog,
            0x0f => Self::SystemBrownOut,
            0x10 => Self::SystemRtcWatchdog,
            0x11 => Self::CpuMainWatchdog1,
            0x12 => Self::SystemSuperWatchdog,
            0x14 => Self::CoreEfuseCrc,
            0x15 => Self::CoreUsbUart,
            0x16 => Self::CoreUsbJtag,
            0x18 => Self::CpuJtag,
            0x19 => Self::CorePowerGlitch,
            0x1a => Self::CpuLockup,
            other => Self::Other(other),
        }
    }

    /// The code of this cause.
    pub const fn code(self) -> u8 {
        match self {
            Self::ChipPowerOn => 0x01,
            Self::CoreSoftware => 0x03,
            Self::CoreDeepSleep => 0x05,
            Self::CoreMainWatchdog0 => 0x07,
            Self::CoreMainWatchdog1 => 0x08,
            Self::CoreRtcWatchdog => 0x09,
            Self::CpuMainWatchdog0 => 0x0b,
            Self::CpuSoftware => 0x0c,
            Self::CpuRtcWatchdog => 0x0d,
            Self::SystemBrownOut => 0x0f,
            Self::SystemRtcWatchdog => 0x10,
            Self::CpuMainWatchdog1 => 0x11,
            Self::SystemSuperWatchdog => 0x12,
            Self::CoreEfuseCrc => 0x14,
            Self::CoreUsbUart => 0x15,
            Self::CoreUsbJtag => 0x16,
            Self::CpuJtag => 0x18,
            Self::CorePowerGlitch => 0x19,
            Self::CpuLockup => 0x1a,
            Self::Other(code) => code,
        }
    }
}

#[cfg(test)]
mod tests;
