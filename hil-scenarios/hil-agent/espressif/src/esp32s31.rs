//! The HIL console over the library's USB console on the ESP32-S31.

use core::{convert::Infallible, future::Future};

use oer_espressif::{AppResult, Error, Evidence, ResetCause, UsbConsole};
use oer_hil_agent::base::{Intake, Requests};
use oer_hil_protocol::{
    Message, RequestIdentity,
    base::{
        BootEvidence, ImageKeySet, PlatformPanic, PostMortemCheckpoints, RejectReason, Rejected,
        ResetReason,
    },
};

use crate::Outbox;

/// The notes and messages the console holds until it writes them.
const NOTES: usize = 8;
const MESSAGES: usize = 8;

/// Writes a note through the early USB writer, until the console opens.
struct EarlyUsb;

impl oer_hil_agent::console::Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> oer_diagnostics::Written {
        oer_esp32s31_compose::console::early(note)
    }

    fn take_lost() -> u32 {
        oer_esp32s31_compose::console::take_early_lost()
    }
}

type HilConsole = oer_hil_agent::console::Console<NOTES, MESSAGES, EarlyUsb>;

oer_espressif_staged_runtime::placed_static! {
    static CONSOLE: HilConsole = oer_hil_agent::console::Console::new(), in critical_data "logging";
}

/// What the image announces in its Hello: the keys of its services and the
/// largest request payload it takes.
#[derive(Clone, Copy)]
pub struct Image {
    pub keys: ImageKeySet<'static>,
    pub maximum_payload_bytes: u16,
}

/// Publish `body` as the answer to `identity`, without waiting.
pub fn answer<M: Message>(identity: RequestIdentity, body: &M) {
    let _sequence = CONSOLE.publish(identity.session_id, identity.request_id, body);
}

/// Serve the host over `console` until the chip resets: the base module
/// from this boot's identity and `evidence`, each request of the image's
/// services handed to the worker through `outbox` (or refused as busy),
/// and `worker`, which answers them, side by side.
///
/// # Errors
/// [`Error::Console`] when the console's transport broke its contract.
pub async fn serve<R: Requests>(
    console: UsbConsole,
    evidence: Evidence,
    image: Image,
    outbox: Outbox<'_, R>,
    worker: impl Future<Output = Infallible>,
) -> AppResult {
    let boot = oer_espressif::boot_id();
    CONSOLE.start(boot);
    let intake = Intake::new(
        boot,
        image.keys,
        image.maximum_payload_bytes,
        oer_espressif_staged_runtime::layout::payload_crc32(),
    );
    let platform = Platform(evidence);
    let serve = CONSOLE.run(console, intake, &platform, |identity, request: R| {
        if outbox.hand(identity, request).is_err() {
            answer(identity, &Rejected(RejectReason::Busy));
        }
    });
    match embassy_futures::select::select(serve, worker).await {
        embassy_futures::select::Either::First(_fault) => Err(Error::Console),
        embassy_futures::select::Either::Second(never) => match never {},
    }
}

/// The boot evidence the base module serves, as the library read it.
struct Platform(Evidence);

impl oer_hil_agent::base::Platform for Platform {
    fn boot_evidence(&self) -> BootEvidence {
        let cause = self.0.reset_cause;
        BootEvidence {
            reset_reason: reset_reason(cause),
            raw_reset_reason: cause.code(),
            post_mortem: None,
            platform_panic: self.0.previous_panic.as_ref().map(|record| PlatformPanic {
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

    /// The library printed and cleared the previous boot's panic before the
    /// boot.
    fn boot_evidence_written(&self) {}

    /// Every page is empty: the library keeps no post-mortem checkpoints.
    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}

fn reset_reason(cause: ResetCause) -> ResetReason {
    match cause {
        ResetCause::CoreSoftware => ResetReason::Software,
        ResetCause::CoreMainWatchdog1 => ResetReason::MainWatchdog1,
        ResetCause::ChipPowerOn => ResetReason::PowerOn,
        ResetCause::SystemBrownOut => ResetReason::Brownout,
        ResetCause::CoreMainWatchdog0 => ResetReason::MainWatchdog0,
        ResetCause::CoreRtcWatchdog
        | ResetCause::CpuRtcWatchdog
        | ResetCause::SystemRtcWatchdog => ResetReason::RtcWatchdog,
        ResetCause::SystemSuperWatchdog => ResetReason::SuperWatchdog,
        ResetCause::CoreUsbJtag | ResetCause::CoreUsbUart => ResetReason::UsbSerialJtag,
        ResetCause::CpuJtag => ResetReason::Jtag,
        ResetCause::CpuLockup => ResetReason::CpuLockup,
        _ => ResetReason::Other,
    }
}
