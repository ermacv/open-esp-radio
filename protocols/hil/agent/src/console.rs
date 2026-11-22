//! The target's one HIL console, held by its one task.
//!
//! Every chip and image uses this console. The chip supplies its byte
//! transport (one duplex `embedded-io-async` endpoint) and the early writer
//! of its USB FIFO for the time before the transport owns the peripheral;
//! the image supplies its [`crate::base::Platform`] and its worker, which
//! serves the image's own requests. The console's state is a [`Writer`]
//! the image places once and lends to [`run`], the one task that writes:
//! the encoder, the sequence it numbers messages with as it writes them and
//! the counts the link's health reports. Nobody publishes into it.
//!
//! [`run`] writes Hello first. It answers each base-module request in the
//! step that decoded it, hands each request of the image to the worker
//! through the [`Mailbox`](crate::mailbox::Mailbox), and writes the
//! worker's answer when it comes back: one request is in work at a time,
//! and another meanwhile is refused `Rejected(Busy)`. An answer the worker
//! could not encode, or a reply it dropped, is refused to the host as
//! `Rejected(Internal)` and counted. What follows an answer's write
//! ([`After`]) is done here, after the write: a trap ends the run. A note is
//! a typed [`oer_portable_diagnostics::Note`]; the console writes one only
//! for the early writer's losses. The console formats no text.

use embassy_futures::select::{Either, select};
use embedded_io_async::{Read, Write};
use oer_portable_diagnostics::{MAX_NOTE_LINE, Note, emit_note};
use oer_protocols_hil_protocol::base::{ImageKeySet, RejectReason, Rejected};
use oer_protocols_hil_protocol::{FrameEncoder, Message, Outbound, RequestIdentity};

use crate::base::{Answer, Intake, Platform, Request, Requests, Sent};
use crate::mailbox::{After, Answered, Console, Outcome};

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
    /// An answer asked for a trap and was written whole.
    Trap(T),
}

/// The boot's first message: the host's handshake expects Hello here.
const HELLO_SEQUENCE: u32 = 0;
/// Bytes read from the endpoint at once.
const RX_CHUNK: usize = 128;

/// How a chip writes a note before its transport owns the USB peripheral:
/// the early writer, which drops what the FIFO has no room for and counts it.
/// A type, not a pointer, so that the call is a direct one.
pub trait Immediate {
    /// The bytes the early writer dropped since the last call.
    fn take_lost() -> u32;
}

/// Whether a request of the image is with the worker.
#[derive(Clone, Copy)]
enum Work {
    Idle,
    Handed,
}

/// The console's state: the encoder, the sequence it numbers with and its
/// counts. Only [`run`] touches it; the image places it once and lends it.
pub struct Writer {
    encoder: FrameEncoder,
    sequence: u32,
    sent_frames: u32,
    /// Messages lost: not encoded, not written whole, or the worker's
    /// answers it could not encode or dropped.
    dropped: u32,
    /// The console's notes not written whole.
    notes_dropped: u32,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub const fn new() -> Self {
        Self {
            encoder: FrameEncoder::new(),
            sequence: HELLO_SEQUENCE,
            sent_frames: 0,
            dropped: 0,
            notes_dropped: 0,
        }
    }

    /// The counts the link's health reports.
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

    /// Writes `body` as the answer to `request`; whether it was written
    /// whole.
    async fn reply<M: Message>(
        &mut self,
        tx: &mut impl Write,
        boot: u64,
        request: RequestIdentity,
        body: &M,
        platform: &impl Platform,
    ) -> bool {
        match Outbound::new(0, request.session_id, request.request_id, body) {
            Ok(message) => self.write(tx, boot, message, platform).await,
            Err(_) => {
                self.lose();
                false
            }
        }
    }

    /// Writes the base module's answer to `request` for an image serving
    /// `image_keys` on `platform`.
    async fn answer(
        &mut self,
        tx: &mut impl Write,
        boot: u64,
        request: RequestIdentity,
        answer: Answer,
        image_keys: ImageKeySet<'_>,
        platform: &impl Platform,
    ) {
        let _written = match answer {
            Answer::Hello(hello) => self.reply(tx, boot, request, &hello, platform).await,
            Answer::ImageKeys(first) => {
                self.reply(tx, boot, request, &image_keys.page(first), platform)
                    .await
            }
            Answer::Boot => {
                self.reply(tx, boot, request, &platform.boot_evidence(), platform)
                    .await
            }
            Answer::PostMortem(first) => {
                let page = platform.post_mortem_checkpoints(first);
                self.reply(tx, boot, request, &page, platform).await
            }
            Answer::Link(link) => self.reply(tx, boot, request, &link, platform).await,
            Answer::Rejected(reason) => {
                self.reply(tx, boot, request, &Rejected(reason), platform)
                    .await
            }
        };
    }

    /// Writes the worker's answer; the trap it asks for once written whole.
    async fn worker_answer<T>(
        &mut self,
        tx: &mut impl Write,
        boot: u64,
        answered: Answered<T>,
        platform: &impl Platform,
    ) -> Option<T> {
        let Answered { identity, outcome } = answered;
        match outcome {
            Outcome::Encoded(message, after) => {
                let written = self.write(tx, boot, message, platform).await;
                match after {
                    After::Trap(trap) if written => Some(trap),
                    After::Trap(_) | After::Nothing => None,
                }
            }
            Outcome::Unencodable | Outcome::Unanswered => {
                self.lose();
                let refusal = Rejected(RejectReason::Internal);
                let _written = self.reply(tx, boot, identity, &refusal, platform).await;
                None
            }
        }
    }

    /// Numbers `message` and writes it whole; whether it was.
    async fn write(
        &mut self,
        tx: &mut impl Write,
        boot: u64,
        mut message: Outbound,
        platform: &impl Platform,
    ) -> bool {
        message.message_sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let Ok(frame) = self.encoder.encode_outbound(boot, &message) else {
            self.lose();
            return false;
        };
        if write_whole(tx, frame).await.is_ok() && write_whole(tx, b"\r\n").await.is_ok() {
            self.sent_frames = self.sent_frames.wrapping_add(1);
            if message.key == oer_protocols_hil_protocol::base::BootEvidence::KEY {
                platform.boot_evidence_written();
            }
            true
        } else {
            self.lose();
            false
        }
    }

    /// Writes one note as a whole line.
    async fn write_note(&mut self, tx: &mut impl Write, note: &Note<'_>) {
        let mut line = NoteLine::new();
        line.encode(note);
        if write_whole(tx, line.framed()).await.is_err() {
            self.notes_dropped = self.notes_dropped.wrapping_add(1);
        }
    }
}

/// Serves the endpoint for the boot `boot` until the chip resets: writes
/// Hello, answers the base module's requests as they are decoded, hands the
/// image's requests to the worker through `mailbox` one at a time and
/// writes their answers. Ends with a fault retaining the endpoint on a
/// broken read contract, or with the trap an answer asked for once it is
/// written.
pub async fn run<P: Immediate, C: Requests, E: Read + Write>(
    writer: &mut Writer,
    mailbox: &mut Console<'_, C>,
    boot: u64,
    mut endpoint: E,
    mut intake: Intake<'_>,
    platform: &impl Platform,
) -> Ended<E, C::Trap> {
    let image_keys = intake.image_keys();
    match Outbound::new(HELLO_SEQUENCE, 0, 0, &intake.hello()) {
        Ok(hello) => {
            let _written = writer.write(&mut endpoint, boot, hello, platform).await;
        }
        Err(_) => writer.lose(),
    }
    // What the early writer dropped before it gave up the peripheral: a
    // loss is never silent.
    let lost = P::take_lost();
    if lost != 0 {
        writer
            .write_note(&mut endpoint, &Note::diag_lost(lost))
            .await;
    }
    let mut work = Work::Idle;
    let mut chunk = [0_u8; RX_CHUNK];
    loop {
        let read = select(mailbox.answers.ready(), endpoint.read(&mut chunk)).await;
        match read {
            Either::First(()) => {
                if let Some(answered) = mailbox.answers.try_receive() {
                    work = Work::Idle;
                    if let Some(trap) = writer
                        .worker_answer(&mut endpoint, boot, answered, platform)
                        .await
                    {
                        return Ended::Trap(trap);
                    }
                }
            }
            Either::Second(Err(_)) => {}
            Either::Second(Ok(length)) => {
                let Some(bytes) = chunk.get(..length) else {
                    return Ended::Fault(RunFault {
                        cause: ConsoleFault::ReadLength {
                            length,
                            capacity: chunk.len(),
                        },
                        endpoint,
                    });
                };
                for &byte in bytes {
                    let Some(request) = intake.receive_byte::<C>(byte, writer.sent()) else {
                        continue;
                    };
                    match request {
                        Request::Answer(identity, answer) => {
                            writer
                                .answer(&mut endpoint, boot, identity, answer, image_keys, platform)
                                .await;
                        }
                        Request::Serve(identity, own) => {
                            let handed = match work {
                                Work::Idle => mailbox.requests.try_send((identity, own)).is_ok(),
                                Work::Handed => false,
                            };
                            if handed {
                                work = Work::Handed;
                            } else {
                                let busy = Rejected(RejectReason::Busy);
                                let _written = writer
                                    .reply(&mut endpoint, boot, identity, &busy, platform)
                                    .await;
                            }
                        }
                    }
                }
            }
        }
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
