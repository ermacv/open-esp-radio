//! The target's one HIL console, held by its one task.
//!
//! Every chip and image uses this console. The chip supplies its byte
//! transport (one duplex `embedded-io-async` endpoint) and the early writer
//! of its USB FIFO for the time before the transport owns the peripheral;
//! the image supplies its [`crate::base::Platform`] and its handlers: what
//! observes and what commands ([`crate::serve`]), two values split once at
//! its composition. The console's state is a [`Writer`] the image places once
//! and lends to [`run`]: the encoder, the sequence it numbers messages with as
//! it writes them and the counts the link's health reports.
//!
//! [`run`] writes Hello first. It answers each base-module request, and each
//! observation of the image, in the step that decoded it. A command of the
//! image is run on the commanding value: the console awaits it beside its
//! read of the endpoint and writes its answer when it comes, still answering
//! the base module and the observations meanwhile; a second command meanwhile
//! is refused `Rejected(Busy)`. Every answer is serialized once, from a
//! reference, into the encoder's buffer. A command that gave no answer, or an
//! answer that cannot be encoded, is refused to the host as
//! `Rejected(Internal)` and counted. What follows a command's write
//! ([`After`]) is done here, after the write: a trap ends the run. A note is a
//! typed [`oer_portable_diagnostics::Note`]; the console writes one only for
//! the early writer's losses. The console formats no text.

use core::pin::pin;

use embassy_futures::select::{Either, select};
use embedded_io_async::{Read, Write};
use oer_portable_diagnostics::{MAX_NOTE_LINE, Note, emit_note};
use oer_protocols_hil_protocol::base::{ImageKeySet, RejectReason, Rejected};
use oer_protocols_hil_protocol::{FrameEncoder, RequestIdentity};

use crate::base::{Answer, Intake, Platform, Request, Sent};
use crate::serve::{After, Answer as _, Frame, Run, Serve, Served, Written};

/// A broken transport: it answered a read longer than the buffer it was
/// given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleFault {
    ReadLength { length: usize, capacity: usize },
}

/// The transport stays owned by the failure until the image records and resets.
pub struct RunFault<E> {
    pub cause: ConsoleFault,
    pub endpoint: E,
}

/// How a run of the console ends.
pub enum Ended<E, T> {
    /// The transport broke its contract.
    Fault(RunFault<E>),
    /// A command's answer asked for a trap and was written whole.
    Trap(T),
}

/// Bytes read from the endpoint at once.
const RX_CHUNK: usize = 128;

/// How a chip writes a note before its transport owns the USB peripheral:
/// the early writer, which drops what the FIFO has no room for and counts it.
/// A type, not a pointer, so that the call is a direct one.
pub trait Immediate {
    /// The bytes the early writer dropped since the last call.
    fn take_lost() -> u32;
}

/// The encoder and the sequence it numbers messages with.
struct Frames {
    encoder: FrameEncoder,
    sequence: u32,
}

impl Frames {
    fn frame(&mut self, boot: u64, request: RequestIdentity) -> Frame<'_> {
        Frame::new(&mut self.encoder, &mut self.sequence, boot, request)
    }
}

/// What the link's health reports of the console's writes.
struct Counts {
    sent_frames: u32,
    /// Messages lost: not encoded, not written whole, or commands that gave
    /// no answer.
    dropped: u32,
    /// The console's notes not written whole.
    notes_dropped: u32,
}

impl Counts {
    const fn sent(&self) -> Sent {
        Sent {
            frames: self.sent_frames,
            dropped: self.dropped,
            notes_dropped: self.notes_dropped,
        }
    }

    fn lose(&mut self) {
        self.dropped = self.dropped.wrapping_add(1);
    }
}

/// The console's state: the encoder, the sequence it numbers with and its
/// counts. Only [`run`] touches it; the image places it once and lends it.
pub struct Writer {
    frames: Frames,
    counts: Counts,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// How an answer's frame ended.
enum Delivery {
    Whole,
    /// It could not be encoded: nothing was written.
    Unencoded,
    /// The transport did not take it whole.
    Broken,
}

impl Writer {
    pub const fn new() -> Self {
        Self {
            frames: Frames {
                encoder: FrameEncoder::new(),
                sequence: 0,
            },
            counts: Counts {
                sent_frames: 0,
                dropped: 0,
                notes_dropped: 0,
            },
        }
    }
}

/// Writes an encoded frame whole, counting it or its loss.
async fn send(counts: &mut Counts, tx: &mut impl Write, written: Written<'_>) -> Delivery {
    let Ok(frame) = written.0 else {
        counts.lose();
        return Delivery::Unencoded;
    };
    if write_whole(tx, frame).await.is_ok() && write_whole(tx, b"\r\n").await.is_ok() {
        counts.sent_frames = counts.sent_frames.wrapping_add(1);
        Delivery::Whole
    } else {
        counts.lose();
        Delivery::Broken
    }
}

/// Writes `Rejected(reason)` as the answer to `request`.
async fn reject(
    writer: &mut Writer,
    tx: &mut impl Write,
    boot: u64,
    request: RequestIdentity,
    reason: RejectReason,
) {
    let written = writer.frames.frame(boot, request).write(&Rejected(reason));
    let _sent = send(&mut writer.counts, tx, written).await;
}

/// The base module's answer to `request` for an image serving `image_keys`
/// on `platform`, serialized from its reference. A function of its own, so
/// that the dispatch on the answer is not a state of the console's future.
fn base_frame<'f>(
    frame: Frame<'f>,
    answer: &Answer,
    image_keys: ImageKeySet<'_>,
    platform: &impl Platform,
) -> Written<'f> {
    match answer {
        Answer::Hello(hello) => frame.write(hello),
        Answer::ImageKeys(first) => frame.write(&image_keys.page(*first)),
        Answer::Boot => frame.write(&platform.boot_evidence()),
        Answer::PostMortem(first) => frame.write(&platform.post_mortem_checkpoints(*first)),
        Answer::Link(link) => frame.write(link),
        Answer::Rejected(reason) => frame.write(&Rejected(*reason)),
    }
}

/// What the console's step knows besides its state: this boot, the image's
/// keys and its platform.
struct Base<'a, P> {
    boot: u64,
    image_keys: ImageKeySet<'a>,
    platform: &'a P,
}

/// Whether a command is in work.
#[derive(Clone, Copy)]
enum Work {
    Idle,
    Busy,
}

/// What feeding bytes came to.
enum Fed<C> {
    /// Every byte was taken.
    Done,
    /// A command was decoded at this many bytes in, while none was in work.
    Command(RequestIdentity, C, usize),
}

/// Feeds `bytes` to the intake, answering the base module and the image's
/// observations as they are decoded; a command of the image is given back
/// while idle and refused busy while one is in work.
async fn feed<P: Platform, R: Serve<O>, O, E: Write>(
    writer: &mut Writer,
    observer: &mut O,
    tx: &mut E,
    intake: &mut Intake<'_>,
    base: &Base<'_, P>,
    bytes: &[u8],
    work: Work,
) -> Fed<R::Command> {
    for (index, &byte) in bytes.iter().enumerate() {
        let Some(request) = intake.receive_byte::<R>(byte, writer.counts.sent()) else {
            continue;
        };
        match request {
            Request::Answer(identity, answer) => {
                let frame = writer.frames.frame(base.boot, identity);
                let written = base_frame(frame, &answer, base.image_keys, base.platform);
                let sent = send(&mut writer.counts, tx, written).await;
                if let (Delivery::Whole, Answer::Boot) = (sent, &answer) {
                    base.platform.boot_evidence_written();
                }
            }
            Request::Serve(identity, own) => {
                match own.serve(observer, writer.frames.frame(base.boot, identity)) {
                    Served::Observed(written) => {
                        if let Delivery::Unencoded = send(&mut writer.counts, tx, written).await {
                            reject(writer, tx, base.boot, identity, RejectReason::Internal).await;
                        }
                    }
                    Served::Command(command) => match work {
                        Work::Idle => {
                            return Fed::Command(identity, command, index.saturating_add(1));
                        }
                        Work::Busy => {
                            reject(writer, tx, base.boot, identity, RejectReason::Busy).await;
                        }
                    },
                }
            }
        }
    }
    Fed::Done
}

/// Serves the endpoint for the boot `boot` until the chip resets: writes
/// Hello, answers the base module's requests and the image's observations
/// (on `observer`) as they are decoded, runs the image's commands (on
/// `commander`) one at a time and writes their answers. Ends with a fault
/// retaining the endpoint on a broken read contract, or with the trap a
/// command's answer asked for once it is written.
pub async fn run<P: Immediate, R, O, C, E: Read + Write>(
    writer: &mut Writer,
    observer: &mut O,
    commander: &mut C,
    boot: u64,
    mut endpoint: E,
    mut intake: Intake<'_>,
    platform: &impl Platform,
) -> Ended<E, <R::Command as Run<C>>::Trap>
where
    R: Serve<O>,
    R::Command: Run<C>,
{
    let base = Base {
        boot,
        image_keys: intake.image_keys(),
        platform,
    };
    let hello = intake.hello();
    let written = writer
        .frames
        .frame(
            boot,
            RequestIdentity {
                boot_id: boot,
                session_id: 0,
                request_id: 0,
            },
        )
        .write(&hello);
    let _sent = send(&mut writer.counts, &mut endpoint, written).await;
    // What the early writer dropped before it gave up the peripheral: a
    // loss is never silent.
    let lost = P::take_lost();
    if lost != 0 {
        write_note(&mut writer.counts, &mut endpoint, &Note::diag_lost(lost)).await;
    }
    let mut chunk = [0_u8; RX_CHUNK];
    // The bytes of the last read not yet fed.
    let (mut start, mut end) = (0, 0);
    loop {
        // Idle: feed what is left of the last read, then read again.
        let (identity, command) = loop {
            if start == end {
                match endpoint.read(&mut chunk).await {
                    Ok(length) if length <= chunk.len() => (start, end) = (0, length),
                    Ok(length) => {
                        return Ended::Fault(RunFault {
                            cause: ConsoleFault::ReadLength {
                                length,
                                capacity: chunk.len(),
                            },
                            endpoint,
                        });
                    }
                    Err(_) => continue,
                }
            }
            let Some(bytes) = chunk.get(start..end) else {
                start = end;
                continue;
            };
            match feed::<_, R, O, E>(
                writer,
                observer,
                &mut endpoint,
                &mut intake,
                &base,
                bytes,
                Work::Idle,
            )
            .await
            {
                Fed::Done => start = end,
                Fed::Command(identity, command, used) => {
                    start = start.saturating_add(used).min(end);
                    break (identity, command);
                }
            }
        };
        // A command in work: it borrows the commanding value alone, while the
        // observations and the base module are answered beside it.
        let mut work = pin!(command.run(commander));
        let ran = loop {
            if let Some(bytes) = chunk.get(start..end).filter(|bytes| !bytes.is_empty()) {
                let _fed = feed::<_, R, O, E>(
                    writer,
                    observer,
                    &mut endpoint,
                    &mut intake,
                    &base,
                    bytes,
                    Work::Busy,
                )
                .await;
                start = end;
            }
            match select(work.as_mut(), endpoint.read(&mut chunk)).await {
                Either::First(ran) => break ran,
                Either::Second(Ok(length)) if length <= chunk.len() => (start, end) = (0, length),
                Either::Second(Ok(length)) => {
                    return Ended::Fault(RunFault {
                        cause: ConsoleFault::ReadLength {
                            length,
                            capacity: chunk.len(),
                        },
                        endpoint,
                    });
                }
                Either::Second(Err(_)) => {}
            }
        };
        match ran {
            Ok((answer, after)) => {
                let written = answer.write(writer.frames.frame(boot, identity));
                match send(&mut writer.counts, &mut endpoint, written).await {
                    Delivery::Whole => {
                        if let After::Trap(trap) = after {
                            return Ended::Trap(trap);
                        }
                    }
                    Delivery::Unencoded => {
                        reject(
                            writer,
                            &mut endpoint,
                            boot,
                            identity,
                            RejectReason::Internal,
                        )
                        .await;
                    }
                    Delivery::Broken => {}
                }
            }
            Err(_unanswered) => {
                writer.counts.lose();
                reject(
                    writer,
                    &mut endpoint,
                    boot,
                    identity,
                    RejectReason::Internal,
                )
                .await;
            }
        }
    }
}

/// Writes one note as a whole line.
async fn write_note(counts: &mut Counts, tx: &mut impl Write, note: &Note<'_>) {
    let mut line = NoteLine::new();
    line.encode(note);
    if write_whole(tx, line.framed()).await.is_err() {
        counts.notes_dropped = counts.notes_dropped.wrapping_add(1);
    }
}

/// The transport did not take all the bytes: it failed, or took none of them
/// or more than it was given.
struct Unwritten;

/// Write all of `bytes`.
async fn write_whole(tx: &mut impl Write, mut bytes: &[u8]) -> Result<(), Unwritten> {
    while !bytes.is_empty() {
        let written = tx.write(bytes).await.map_err(|_| Unwritten)?;
        bytes = bytes
            .get(written..)
            .filter(|_| written != 0)
            .ok_or(Unwritten)?;
    }
    Ok(())
}

/// One note as the `OER-DIAG:01` line the host decodes.
struct NoteLine {
    /// The line with its framing: `\r\n` before and after, then a NUL.
    bytes: [u8; MAX_NOTE_LINE + 1],
    length: usize,
}

impl NoteLine {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_NOTE_LINE + 1],
            length: 0,
        }
    }

    /// Numbers `note` from the boot's shared note sequence. A line that
    /// does not fit is left empty, never half.
    fn encode(&mut self, note: &Note<'_>) {
        let written = emit_note(note, |bytes: &[u8]| {
            let end = self.length.checked_add(bytes.len()).ok_or(())?;
            let slot = if end <= MAX_NOTE_LINE {
                self.bytes.get_mut(self.length..end)
            } else {
                None
            };
            slot.map(|slot| {
                slot.iter_mut()
                    .zip(bytes)
                    .for_each(|(to, from)| *to = *from)
            })
            .map(|()| self.length = end)
            .ok_or(())
        });
        if written.is_err() {
            self.length = 0;
        }
    }

    /// The whole line with its framing.
    fn framed(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests;
