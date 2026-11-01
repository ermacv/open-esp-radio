//! The HIL console over the library's USB console on the ESP32-S31.

use core::future::Future;

use oer_espressif::{AppResult, Error, Evidence, ExceptionEvidence, ResetCause, UsbConsole};
use oer_hil_agent::base::{Intake, Requests};
use oer_hil_protocol::{
    Message, RequestIdentity,
    base::{
        BootEvidence, ExceptionFault, Fault, ImageKeySet, PlatformPanic, PostMortemCheckpoints,
        PostMortemSummary, RejectReason, Rejected, ResetReason, TrapLine, TrapObservation,
        TrapState,
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

/// Publish `body` as the answer to `identity`, wait until the console wrote
/// it, then trap on an illegal instruction: the library's exception path
/// records the exception and resets, and the next boot serves the record
/// as its post-mortem. Never returns once the answer is written.
///
/// # Errors
/// [`Error::Console`] when the console could not confirm the write: its
/// barrier's fan-out ([`oer_hil_agent::console::progress::ReceiverLimit`])
/// is exhausted, which its contract makes a terminal fault. The image does
/// not trap; the application ends with the error, which the library records
/// before it resets.
pub async fn answer_then_trap<M: Message>(identity: RequestIdentity, body: &M) -> AppResult {
    let sequence = CONSOLE
        .publish_reliably(identity.session_id, identity.request_id, body)
        .await;
    match CONSOLE.written(sequence).await {
        Ok(()) => oer_riscv::illegal_instruction(),
        Err(_limit) => Err(Error::Console),
    }
}

/// Serve the host over `console` until the chip resets: the base module
/// from this boot's identity and `evidence`, each request of the image's
/// services handed to the worker through `outbox` (or refused as busy),
/// and the worker `start` makes, which answers them, side by side. The
/// worker's future is made only where it runs, so the application's
/// future holds it once.
///
/// # Errors
/// [`Error::Console`] when the console's transport broke its contract, or
/// the error the worker ended with.
pub async fn serve<R: Requests, W: Future<Output = AppResult>>(
    console: UsbConsole,
    evidence: Evidence,
    image: Image,
    outbox: Outbox<'_, R>,
    start: impl FnOnce() -> W,
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
    match embassy_futures::select::select(serve, start()).await {
        embassy_futures::select::Either::First(_fault) => Err(Error::Console),
        embassy_futures::select::Either::Second(ended) => ended,
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
            post_mortem: self.0.previous_exception.as_ref().map(post_mortem),
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

    /// Every page is empty: the post-mortem of an exception holds no
    /// checkpoints.
    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}

/// The previous boot's exception as the base module's post-mortem: the one
/// boot that wrote it, no checkpoints.
fn post_mortem(exception: &ExceptionEvidence) -> PostMortemSummary {
    let trap = exception.trap;
    PostMortemSummary {
        boot_count: 1,
        checkpoints: 0,
        fault: Some(Fault::Exception(ExceptionFault {
            trap: TrapState {
                // A hart number the runtime retained; never above one.
                hart: u8::try_from(trap.hart).unwrap_or(u8::MAX),
                mcause: trap.mcause,
                mepc: trap.mepc,
                mtval: trap.mtval,
                ra: trap.ra,
                sp: trap.sp,
                mscratch: trap.mscratch,
            },
            observed: exception.observed.map(|observed| TrapObservation {
                stopped: observed.stopped,
                mtvec: observed.mtvec,
                mtvt: observed.mtvt,
                pending: observed.pending,
                line: observed.line.map(|line| TrapLine {
                    vector: line.vector,
                    slot: line.slot,
                    state: line.state,
                    routed: line.routed,
                }),
            }),
        })),
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
