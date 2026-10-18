//! Platform observations and diagnostics of the radio-free SoC image.
pub(crate) mod console;
pub(crate) mod postmortem;
mod watchdog;

pub(crate) fn boot_evidence() -> oer_hil_protocol::base::BootEvidence {
    use esp_hal::rtc_cntl::SocResetReason as Soc;
    use oer_hil_protocol::base::{BootEvidence, ResetReason};
    let raw = esp_hal::system::reset_reason();
    BootEvidence {
        // Only reasons that mean the same as on the other chips are named;
        // the raw code keeps the rest, such as a TIMG1 reset of CPU 0 alone.
        reset_reason: match raw {
            Some(Soc::CoreSw) => ResetReason::Software,
            Some(Soc::CoreMwdt1) => ResetReason::MainWatchdog1,
            Some(Soc::ChipPowerOn) => ResetReason::PowerOn,
            Some(Soc::SysBrownOut) => ResetReason::Brownout,
            Some(Soc::CoreMwdt0) => ResetReason::MainWatchdog0,
            Some(Soc::CoreRtcWdt | Soc::Cpu0RtcWdt | Soc::SysRtcWdt) => ResetReason::RtcWatchdog,
            Some(Soc::SysSuperWdt) => ResetReason::SuperWatchdog,
            Some(Soc::CoreUsbJtag | Soc::CoreUsbUart) => ResetReason::UsbSerialJtag,
            Some(Soc::Cpu0JtagCpu) => ResetReason::Jtag,
            _ => ResetReason::Other,
        },
        raw_reset_reason: raw.map_or(0, |reason| reason as u8),
        post_mortem: postmortem::previous(|previous| previous.map(|previous| previous.summary())),
        platform_panic: PLATFORM_PANIC.try_get().copied().flatten().map(|record| {
            oer_hil_protocol::base::PlatformPanic {
                hart: record.hart,
                in_interrupt: record.in_interrupt,
                interrupted_pc: record.interrupted_pc,
                // At most 48 bytes, the protocol's bound.
                file: record.file().try_into().unwrap_or_default(),
                line: record.line,
                column: record.column,
            }
        }),
    }
}

/// The platform panic entry's record of the previous boot, taken once at
/// boot: taking it clears the retained slot.
static PLATFORM_PANIC: embassy_sync::once_lock::OnceLock<
    Option<oer_espressif_staged_runtime::panic::PanicRecord>,
> = embassy_sync::once_lock::OnceLock::new();

/// Take the previous boot's platform panic record; before the first boot
/// evidence is served.
pub(crate) fn take_platform_panic() {
    let _ = PLATFORM_PANIC.init(oer_espressif_staged_runtime::panic::take_previous());
}

/// Checkpoints `first..` of the previous boot's post-mortem.
pub(crate) fn post_mortem_checkpoints(first: u8) -> oer_hil_protocol::base::PostMortemCheckpoints {
    postmortem::previous(|previous| {
        previous.map_or_else(
            || oer_hil_protocol::base::PostMortemCheckpoints {
                first,
                checkpoints: Default::default(),
            },
            |previous| previous.page(first),
        )
    })
}
