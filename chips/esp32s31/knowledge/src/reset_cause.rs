//! Why an HP core last left reset: the codes of the reset-cause field.
//!
//! Each HP core has its own read-only cause word in `LP_AON_CLKRST`; its
//! six-bit cause field (bits 6:1) keeps the code until the next reset.
//!
//! SOURCE: rev0 ROM `esp32s31_rev0_rom.elf` (pin `esp32s31/rom`),
//! `rom[]::rtc_get_reset_reason` at 0x2f805938, read with `cargo blobray
//! function-records` and `register-accesses`: the function takes the core
//! number, loads `0x20701030` (`LP_AON_CLKRST.HPCORE0_RESET_CAUSE`) for core 0
//! and `0x20701034` (`HPCORE1_RESET_CAUSE`) for any other value, and returns
//! bits 6:1 of the word (`srli 1`, `andi 63`); no other bit reaches the result.
//! ESP-IDF 4d80c1b2 `components/soc/esp32s31/register/soc/lp_clkrst_reg.h`
//! places the same two words at `DR_REG_LP_AONCLKRST_BASE` + 0x30 and + 0x34.

/// A reset-cause code, as the cause field holds it.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/soc/esp32s31/include/soc/reset_reasons.h:29-51`
/// (`soc_reset_reason_t`). The levels in the names follow its terminology
/// (lines 9-19): a CPU reset resets one core only; a core reset the digital
/// system except the RTC sub-system; a system reset the digital system with
/// the RTC sub-system; a chip reset the whole chip with its analog part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// 0x01 `CHIP_POWER_ON`: power-on reset.
    ChipPowerOn,
    /// 0x03 `CORE_SW`: software reset of the digital core.
    CoreSoftware,
    /// 0x05 `CORE_DEEP_SLEEP` and `CORE_PMU_PWR_DOWN`, which share the code:
    /// deep sleep or the PMU powering the HP domain down reset the digital
    /// core.
    CorePowerDown,
    /// 0x06 `CPU_PMU_PWR_DOWN`: the PMU powering the HP CPU down reset it.
    CpuPowerDown,
    /// 0x07 `CORE_MWDT0`: main watchdog 0 reset the digital core.
    CoreMainWatchdog0,
    /// 0x08 `CORE_MWDT1`: main watchdog 1 reset the digital core.
    CoreMainWatchdog1,
    /// 0x09 `CORE_RWDT`: the RTC watchdog reset the digital core.
    CoreRtcWatchdog,
    /// 0x0b `CPU_MWDT`: a main watchdog reset the HP CPU.
    CpuMainWatchdog,
    /// 0x0c `CPU_SW` (also `CPU0_SW`): software reset of the HP CPU.
    CpuSoftware,
    /// 0x0d `CPU_RWDT`: the RTC watchdog reset the HP CPU.
    CpuRtcWatchdog,
    /// 0x0f `SYS_BROWN_OUT`: VDD was not stable.
    SystemBrownOut,
    /// 0x10 `SYS_RWDT`: the RTC watchdog reset the system.
    SystemRtcWatchdog,
    /// 0x12 `SYS_SUPER_WDT`: the super watchdog reset the digital core and
    /// the RTC module.
    SystemSuperWatchdog,
    /// 0x13 `CORE_PWR_GLITCH`: a power glitch reset the digital core and the
    /// RTC module.
    CorePowerGlitch,
    /// 0x14 `CORE_EFUSE_CRC`: an eFuse CRC error reset the digital core.
    CoreEfuseCrc,
    /// 0x16 `CORE_USB_JTAG`: the USB Serial/JTAG controller's JTAG reset the
    /// digital core.
    CoreUsbJtag,
    /// 0x17 `CORE_USB_UART`: the USB Serial/JTAG controller's UART reset the
    /// digital core.
    CoreUsbUart,
    /// 0x18 `CPU_JTAG`: a reset command from JTAG.
    CpuJtag,
    /// 0x1a `CPU_LOCKUP`: the CPU locked up (an exception inside the
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
            0x05 => Self::CorePowerDown,
            0x06 => Self::CpuPowerDown,
            0x07 => Self::CoreMainWatchdog0,
            0x08 => Self::CoreMainWatchdog1,
            0x09 => Self::CoreRtcWatchdog,
            0x0b => Self::CpuMainWatchdog,
            0x0c => Self::CpuSoftware,
            0x0d => Self::CpuRtcWatchdog,
            0x0f => Self::SystemBrownOut,
            0x10 => Self::SystemRtcWatchdog,
            0x12 => Self::SystemSuperWatchdog,
            0x13 => Self::CorePowerGlitch,
            0x14 => Self::CoreEfuseCrc,
            0x16 => Self::CoreUsbJtag,
            0x17 => Self::CoreUsbUart,
            0x18 => Self::CpuJtag,
            0x1a => Self::CpuLockup,
            other => Self::Other(other),
        }
    }

    /// The code of this cause.
    pub const fn code(self) -> u8 {
        match self {
            Self::ChipPowerOn => 0x01,
            Self::CoreSoftware => 0x03,
            Self::CorePowerDown => 0x05,
            Self::CpuPowerDown => 0x06,
            Self::CoreMainWatchdog0 => 0x07,
            Self::CoreMainWatchdog1 => 0x08,
            Self::CoreRtcWatchdog => 0x09,
            Self::CpuMainWatchdog => 0x0b,
            Self::CpuSoftware => 0x0c,
            Self::CpuRtcWatchdog => 0x0d,
            Self::SystemBrownOut => 0x0f,
            Self::SystemRtcWatchdog => 0x10,
            Self::SystemSuperWatchdog => 0x12,
            Self::CorePowerGlitch => 0x13,
            Self::CoreEfuseCrc => 0x14,
            Self::CoreUsbJtag => 0x16,
            Self::CoreUsbUart => 0x17,
            Self::CpuJtag => 0x18,
            Self::CpuLockup => 0x1a,
            Self::Other(code) => code,
        }
    }
}

#[cfg(test)]
mod tests;
