//! The boot evidence the base module serves: why the chip left reset and
//! the platform panic record of the previous boot, both read once at boot
//! and owned by the console's platform. The image keeps no post-mortem
//! checkpoints.

use oer_esp32s31_hal::peripherals::reset_cause::{ResetCause as Cause, ResetCauses};
use oer_espressif_staged_runtime::panic::PanicRecord;
use oer_hil_agent::base::Platform;
use oer_hil_protocol::base::{BootEvidence, PlatformPanic, PostMortemCheckpoints, ResetReason};

/// What the boot read before the console serves its evidence.
pub(crate) struct Evidence {
    causes: ResetCauses,
    platform_panic: Option<PanicRecord>,
}

impl Evidence {
    /// Read the reset causes from their transferred owner and peek the
    /// previous boot's platform panic record, which only complete output
    /// clears.
    pub(crate) fn read(
        registers: oer_esp32s31_hal::peripherals::reset_cause::ResetCauseRegisters,
    ) -> Self {
        Self {
            causes: oer_esp32s31_compose::reset_causes::read(registers),
            platform_panic: oer_espressif_staged_runtime::panic::peek_previous(),
        }
    }
}

impl Platform for Evidence {
    fn boot_evidence(&self) -> BootEvidence {
        // The console serves boot evidence from core 0.
        let cause = self.causes.of(oer_esp32s31_hal::platform::BOOT_HART);
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
            post_mortem: None,
            platform_panic: self.platform_panic.as_ref().map(|record| PlatformPanic {
                hart: record.hart,
                in_interrupt: record.in_interrupt,
                interrupted_pc: record.interrupted_pc,
                // At most 48 bytes, the protocol's bound.
                file: record.file().try_into().unwrap_or_default(),
                line: record.line,
                column: record.column,
            }),
        }
    }

    fn boot_evidence_written(&self) {
        if let Some(record) = &self.platform_panic {
            oer_espressif_staged_runtime::panic::clear_previous(record);
        }
    }

    /// Every page is empty: the image keeps no post-mortem checkpoints.
    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}
