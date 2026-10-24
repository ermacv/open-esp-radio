//! Platform observations and diagnostics, independent of radio protocols.
#[cfg(feature = "system-watchdog")]
pub(super) mod console;
#[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
pub(crate) mod panic;
#[cfg(feature = "system-panic-reset")]
pub(super) mod panic_reset;
pub(crate) mod postmortem;
#[cfg(feature = "system-watchdog")]
mod watchdog;

pub(super) fn boot_evidence() -> oer_hil_protocol::base::BootEvidence {
    use oer_esp32s31_hal::platform::{HpCore, ResetCause as Cause};
    use oer_hil_protocol::base::{BootEvidence, ResetReason};
    // The console serves boot evidence from core 0.
    let cause = RESET_CAUSES
        .try_get()
        .expect("the boot reads the reset causes before serving evidence")
        .of(HpCore::Core0);
    BootEvidence {
        reset_reason: match cause {
            Cause::CoreSoftware => ResetReason::Software,
            Cause::CoreMainWatchdog1 => ResetReason::MainWatchdog1,
            Cause::ChipPowerOn => ResetReason::PowerOn,
            Cause::SystemBrownOut => ResetReason::Brownout,
            Cause::CoreMainWatchdog0 => ResetReason::MainWatchdog0,
            Cause::CoreRtcWatchdog | Cause::CpuRtcWatchdog | Cause::SystemRtcWatchdog => {
                ResetReason::RtcWatchdog
            }
            Cause::SystemSuperWatchdog => ResetReason::SuperWatchdog,
            Cause::CoreUsbJtag | Cause::CoreUsbUart => ResetReason::UsbSerialJtag,
            Cause::CpuJtag => ResetReason::Jtag,
            Cause::CpuLockup => ResetReason::CpuLockup,
            _ => ResetReason::Other,
        },
        raw_reset_reason: cause.code(),
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

/// Why each HP core last left reset, read once at boot.
static RESET_CAUSES: embassy_sync::once_lock::OnceLock<oer_esp32s31_hal::platform::ResetCauses> =
    embassy_sync::once_lock::OnceLock::new();

/// Read the reset causes from their transferred owner; before the first boot
/// evidence is served.
pub(crate) fn read_reset_causes(registers: oer_esp32s31_hal::platform::ResetCauseRegisters) {
    let _ = RESET_CAUSES.init(oer_esp32s31_hal::platform::ResetCauses::read(registers));
}

/// Take the previous boot's platform panic record; before the first boot
/// evidence is served.
pub(crate) fn take_platform_panic() {
    let _ = PLATFORM_PANIC.init(oer_espressif_staged_runtime::panic::take_previous());
}

/// Checkpoints `first..` of the previous boot's post-mortem.
pub(super) fn post_mortem_checkpoints(first: u8) -> oer_hil_protocol::base::PostMortemCheckpoints {
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
