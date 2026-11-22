//! The HIL console over the platform runtime's USB console on the
//! ESP32-S31: the one mount of probes and of images on the entry alike.

use core::{convert::Infallible, future::Future};

use oer_chips_esp32s31_platform_runtime::{exception::Observed, reset_causes::ResetCause};
use oer_chips_family_platform_runtime::{panic::PanicRecord, trap::PreviousException};
use oer_protocols_hil_agent::base::{Intake, Requests};
use oer_protocols_hil_agent::console::{self, Writer};
use oer_protocols_hil_protocol::{
    Message, RequestIdentity,
    base::{
        BootEvidence, ExceptionFault, Fault, ImageKeySet, PlatformPanic, PostMortemCheckpoints,
        PostMortemSummary, ResetReason, TrapLine, TrapObservation, TrapState,
    },
};

use crate::{DEPTH, LaneDrain, Outbox, Producer};

/// What the console of an image is made of: the mailbox's console end, the
/// writer's place and its lanes' writer ends.
pub struct Mount<'a, R, const K: usize> {
    pub outbox: Outbox<'a, R>,
    pub writer: &'a mut Writer,
    pub drains: [LaneDrain; K],
}

/// What the early USB writer dropped before the console took the
/// peripheral.
struct EarlyUsb;

impl console::Immediate for EarlyUsb {
    fn take_lost() -> u32 {
        oer_chips_esp32s31_platform_runtime::console::take_early_lost()
    }
}

/// What the image announces in its Hello: the keys of its services and the
/// largest request payload it takes.
#[derive(Clone, Copy)]
pub struct Image {
    pub keys: ImageKeySet<'static>,
    pub maximum_payload_bytes: u16,
}

/// Publish `body` as the answer to `identity` through the context's own
/// `producer`, without waiting; a full lane drops it and counts the loss in
/// the link's health.
pub fn answer<M: Message>(producer: &mut Producer, identity: RequestIdentity, body: &M) {
    let _counted = producer.publish(identity.session_id, identity.request_id, body);
}

/// What the image read before its boot: the base module's boot evidence.
#[derive(Clone, Copy, Debug)]
pub struct Records {
    /// Why hart 0 left reset.
    pub reset_cause: ResetCause,
    /// The previous boot's panic, already printed and cleared.
    pub previous_panic: Option<PanicRecord>,
    /// The previous boot's CPU exception, with what the exception hook read
    /// after the stop when a record of it follows this trap.
    pub previous_exception: Option<(PreviousException, Option<Observed>)>,
}

/// Publish `body` as the answer to `identity`, wait until the console wrote
/// it, then trap on an illegal instruction: the runtime's exception path
/// records the exception and resets, and the next boot serves the record
/// as its post-mortem. Never returns once the answer is written.
///
/// # Errors
/// `console_fault` when the answer could not be built: nothing was queued.
/// The image does not trap; it ends with the error.
pub async fn answer_then_trap<M: Message, E>(
    producer: &mut Producer,
    identity: RequestIdentity,
    body: &M,
    console_fault: E,
) -> Result<Infallible, E> {
    let Ok(entered) = producer
        .publish_reliably(identity.session_id, identity.request_id, body)
        .await
    else {
        return Err(console_fault);
    };
    producer.written(entered).await;
    oer_portable_riscv::illegal_instruction()
}

/// Serve the host over `console` (the image's USB console: the platform
/// runtime's, or a probe's own endpoint) until the chip resets: the base module
/// from this boot's identity `boot` and `records`, each request of the
/// image's services handed to the worker through `outbox` (or refused as
/// busy), and the worker `start` makes, side by side. The image places the
/// console's writer and lanes once (`mount`); the drains are its lanes'
/// writer ends, whose producers its contexts hold and answer through. The worker's
/// future is made only where it runs, so the image's future holds it once.
///
/// # Errors
/// `console_fault` when the console's transport broke its contract, or the
/// error the worker ended with.
pub async fn serve<
    C: embedded_io_async::Read + embedded_io_async::Write,
    R: Requests,
    E,
    W: Future<Output = Result<Infallible, E>>,
    const K: usize,
>(
    console: C,
    boot: u64,
    records: Records,
    image: Image,
    mount: Mount<'_, R, K>,
    console_fault: E,
    start: impl FnOnce() -> W,
) -> Result<Infallible, E> {
    let intake = Intake::new(
        boot,
        image.keys,
        image.maximum_payload_bytes,
        oer_chips_family_platform_runtime::layout::payload_crc32(),
    );
    let platform = Platform(records);
    let Mount {
        outbox,
        writer,
        mut drains,
    } = mount;
    let serve = console::run::<EarlyUsb, R, C, K, DEPTH>(
        writer,
        &mut drains,
        boot,
        console,
        intake,
        &platform,
        |identity, request: R| {
            outbox
                .hand(identity, request)
                .map_err(|crate::Busy| console::Busy)
        },
    );
    match embassy_futures::select::select(serve, start()).await {
        embassy_futures::select::Either::First(_fault) => Err(console_fault),
        embassy_futures::select::Either::Second(ended) => ended,
    }
}

/// The boot evidence the base module serves, as the image read it.
struct Platform(Records);

impl oer_protocols_hil_agent::base::Platform for Platform {
    fn boot_evidence(&self) -> BootEvidence {
        let cause = self.0.reset_cause;
        BootEvidence {
            reset_reason: reset_reason(cause),
            raw_reset_reason: cause.code(),
            post_mortem: self
                .0
                .previous_exception
                .as_ref()
                .map(|(trap, observed)| post_mortem(trap, observed.as_ref())),
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

    /// The image printed and cleared the previous boot's panic before the
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
fn post_mortem(trap: &PreviousException, observed: Option<&Observed>) -> PostMortemSummary {
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
            observed: observed.map(|observed| TrapObservation {
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
