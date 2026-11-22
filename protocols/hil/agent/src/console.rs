//! The target's one HIL console.
//!
//! Every chip and image uses this console. The chip supplies its byte
//! transport (one duplex `embedded-io-async` endpoint) and the early writer
//! of its USB FIFO for the time before the transport owns the peripheral; the image supplies its [`crate::base::Platform`] and
//! serves its own requests. The console owns everything between: the ordered
//! queue of outgoing messages, the queue of diagnostic notes, the request
//! intake with the base module, and the link counters.
//!
//! Producers never touch the endpoint. They serialize their message into
//! the queue ([`Console::publish`] drops it when the queue is full,
//! [`Console::publish_reliably`] waits for room first), and the one
//! [`Console::run`] task frames and writes messages and notes in queue order.
//! The writer numbers each message as it takes it out of the queue, so the
//! numbers follow the queue's order by construction, whatever level each
//! producer runs at; a producer only enters its message and takes no
//! number. A producer that must know its message written waits for its
//! place among the entries ([`Entered`], [`Console::written`]).
//!
//! What is in the message queue at once is bounded by the host, not by a
//! type: the host drives one request at a time per session and keeps at
//! most one posted beside it, and an image answers each once, so beside a
//! burst of the image's own events (bounded by its depth, `MESSAGES`) the
//! queue holds the answers of those two requests and of the base module's. A note is a typed
//! [`oer_portable_diagnostics::Note`]; the console numbers it when it writes it, so
//! the host sees a lost one and never a reordered one. The console formats
//! no text.

pub mod progress;

use core::cell::RefCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embedded_io_async::{Read, Write};
use oer_portable_diagnostics::{MAX_NOTE_LINE, Note, Written, emit_note};
use oer_protocols_hil_protocol::base::{ImageKeySet, LinkHealth, Rejected};
use oer_protocols_hil_protocol::{
    DecodeCounters, FrameEncoder, Message, Outbound, RequestIdentity,
};

use crate::base::{Answer, Intake, Platform, Request, Requests, Sent};

/// A broken transport or overlapping access to the private decoder counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleFault {
    ReadLength { length: usize, capacity: usize },
    CountersBorrowed { writing: bool },
}

/// The transport stays owned by the failure until the image records and resets.
pub struct RunFault<E> {
    pub cause: ConsoleFault,
    pub endpoint: E,
}

/// Messages and notes written back to back before the task looks at the
/// queues again.
const DRAIN_BATCH: usize = 4;
/// The boot's first message: the host's handshake expects Hello here.
const HELLO_SEQUENCE: u32 = 0;
/// Bytes read from the endpoint at once.
const RX_CHUNK: usize = 128;

/// How a chip writes a note before its transport owns the USB peripheral:
/// the early writer, which drops what the FIFO has no room for and counts it.
/// A type, not a pointer, so that the call is a direct one.
pub trait Immediate {
    /// Writes `note` now. [`Written::Retired`] when the early writer no
    /// longer owns the peripheral, which the transport has taken; every
    /// other outcome is the early writer's, a dropped one counted by it.
    fn note(note: &Note<'_>) -> Written;

    /// The bytes the early writer dropped since the last call.
    fn take_lost() -> u32;
}

/// A message's place among the queue's entries since the boot's start, for
/// a wait until it is written ([`Console::written`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entered(u32);

impl Entered {
    /// The place, for a record that keeps it.
    pub const fn place(self) -> u32 {
        self.0
    }
}

/// A message lost before the queue: it could not be built, or the queue
/// was full. Counted in the link's health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dropped;

/// One console. `NOTES` bounds the queued notes and `MESSAGES` the queued
/// messages; `P` is the chip's early writer.
pub struct Console<const NOTES: usize, const MESSAGES: usize, P: Immediate> {
    immediate: PhantomData<P>,
    running: AtomicBool,
    boot_low: AtomicU32,
    boot_high: AtomicU32,
    /// The next message's sequence: the writer's alone, taken as it takes
    /// the message out of the queue.
    sequence: AtomicU32,
    /// The messages entered into the queue since the boot's start.
    entries: AtomicU32,
    /// The entries the writer took out of the queue.
    taken: AtomicU32,
    // The one encoder of the message writer, kept here rather than in the
    // writer's future, which is moved into its task.
    encoder: AsyncMutex<CriticalSectionRawMutex, FrameEncoder>,
    messages: Channel<CriticalSectionRawMutex, Outbound, MESSAGES>,
    notes: Channel<CriticalSectionRawMutex, Note<'static>, NOTES>,
    written: progress::SerializedEvents,
    sent_frames: AtomicU32,
    dropped_messages: AtomicU32,
    received: Mutex<CriticalSectionRawMutex, RefCell<DecodeCounters>>,
    lost: NoteLoss,
}

/// What the note queue lost.
struct NoteLoss {
    dropped: AtomicU32,
    write_errors: AtomicU32,
}

impl<const NOTES: usize, const MESSAGES: usize, P: Immediate> Console<NOTES, MESSAGES, P> {
    pub const fn new() -> Self {
        Self {
            immediate: PhantomData,
            running: AtomicBool::new(false),
            boot_low: AtomicU32::new(0),
            boot_high: AtomicU32::new(0),
            sequence: AtomicU32::new(0),
            entries: AtomicU32::new(0),
            taken: AtomicU32::new(0),
            encoder: AsyncMutex::new(FrameEncoder::new()),
            messages: Channel::new(),
            notes: Channel::new(),
            written: progress::SerializedEvents::new(),
            sent_frames: AtomicU32::new(0),
            dropped_messages: AtomicU32::new(0),
            received: Mutex::new(RefCell::new(DecodeCounters {
                frames: 0,
                cobs_errors: 0,
                too_short: 0,
                header_errors: 0,
                framing_version_errors: 0,
                message_kind_errors: 0,
                payload_length_errors: 0,
                checksum_errors: 0,
                deserialize_errors: 0,
                overflows: 0,
            })),
            lost: NoteLoss {
                dropped: AtomicU32::new(0),
                write_errors: AtomicU32::new(0),
            },
        }
    }

    /// Installs the boot's identity before any message is published.
    ///
    /// Sequence 0 is the boot's Hello, which [`Console::run`] writes before
    /// anything queued; messages published earlier take the sequences after
    /// it, so a boot always begins with Hello whichever task publishes first.
    pub fn start(&self, boot_id: u64) {
        self.boot_low.store(boot_id as u32, Ordering::Relaxed);
        self.boot_high
            .store((boot_id >> 32) as u32, Ordering::Relaxed);
        self.sequence.store(HELLO_SEQUENCE + 1, Ordering::Relaxed);
        self.entries.store(0, Ordering::Relaxed);
        self.taken.store(0, Ordering::Relaxed);
        self.written.publish_next(0);
    }

    pub fn boot_id(&self) -> u64 {
        u64::from(self.boot_low.load(Ordering::Acquire))
            | (u64::from(self.boot_high.load(Ordering::Acquire)) << 32)
    }

    /// Queues `body` without making the caller wait, from any level; a full
    /// queue drops it and counts the loss. It gives no place to wait on: a
    /// producer of a level above another's may enter behind that one's
    /// message and count first, so its count would name the message before
    /// its own ([`Console::entered`]).
    ///
    /// # Errors
    /// [`Dropped`] when the message could not be built or the queue was
    /// full: counted in the link's health, no number spent on it.
    pub fn publish<M: Message>(
        &self,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> Result<(), Dropped> {
        let queued = Outbound::new(0, session_id, request_id, body)
            .ok()
            .and_then(|message| self.messages.try_send(message).ok());
        match queued {
            Some(()) => {
                let _place = self.entered();
                Ok(())
            }
            None => Err(self.drop_message()),
        }
    }

    /// Queues `body`, waiting for room: for control-plane boundaries a full
    /// telemetry queue must not erase. High-rate observations use
    /// [`Console::publish`], which never applies backpressure.
    ///
    /// The place it answers is right for a producer on the console's own
    /// level, the lowest that produces: no producer preempts it from below,
    /// so the place is its message's or, when a higher level entered
    /// between its entry and its count, the one after. A wait for room
    /// already binds its caller to that level.
    ///
    /// # Errors
    /// [`Dropped`] when the message could not be built: counted, nothing
    /// queued.
    pub async fn publish_reliably<M: Message>(
        &self,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> Result<Entered, Dropped> {
        let Ok(message) = Outbound::new(0, session_id, request_id, body) else {
            return Err(self.drop_message());
        };
        self.messages.send(message).await;
        Ok(self.entered())
    }

    /// The count of entries so far, taken by a producer just after its own
    /// entry. Between a producer's entry and its count a producer of a
    /// higher level may enter and count: the lower producer's count then
    /// names the message after its own (a wait lasts a message longer), and
    /// the higher producer's names the lower one's message, one before its
    /// own (a wait would end before its message is written). Only a
    /// producer that nothing preempts from below, the console's own level,
    /// may therefore wait on its count ([`Console::publish_reliably`]).
    fn entered(&self) -> Entered {
        Entered(self.entries.fetch_add(1, Ordering::Relaxed))
    }

    /// A message lost before the queue, counted.
    fn drop_message(&self) -> Dropped {
        self.dropped_messages.fetch_add(1, Ordering::Relaxed);
        Dropped
    }

    /// Waits until the writer has written the entries up to `entered`, a
    /// place [`Console::publish_reliably`] gave on the console's own level:
    /// its message, or the one after it, written.
    pub async fn written(&self, entered: Entered) -> Result<(), progress::ReceiverLimit> {
        self.written.wait_for(entered.0).await
    }

    /// Writes `note` through the early writer while it owns the USB
    /// peripheral, and queues it for [`Console::run`] once the transport
    /// does, waiting for room instead of losing it. Only for reporting
    /// outside measured hot paths, which must not depend on USB progress.
    pub async fn note_reliably(&self, note: &Note<'static>) {
        if !self.running.load(Ordering::Acquire) && P::note(note) != Written::Retired {
            return;
        }
        loop {
            core::future::poll_fn(|cx| self.notes.poll_ready_to_send(cx)).await;
            if self.notes.try_send(*note).is_ok() {
                return;
            }
        }
    }

    /// Counts a message its producer could not build.
    pub fn lose_message(&self) {
        self.dropped_messages.fetch_add(1, Ordering::Relaxed);
    }

    /// Reports `note` without waiting: through the early writer while it
    /// owns the USB peripheral, otherwise in the note queue, where a full
    /// queue drops it and counts the loss. For synchronous callers.
    pub fn note_immediately(&self, note: &Note<'static>) {
        if !self.running.load(Ordering::Acquire) && P::note(note) != Written::Retired {
            return;
        }
        if self.notes.try_send(*note).is_err() {
            self.lost.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Notes lost to a full queue or a failed write.
    pub fn dropped_notes(&self) -> u32 {
        self.lost.dropped.load(Ordering::Relaxed)
    }

    fn sent(&self) -> Sent {
        Sent {
            frames: self.sent_frames.load(Ordering::Acquire),
            dropped: self.dropped_messages.load(Ordering::Acquire),
            notes_dropped: self.dropped_notes(),
        }
    }

    /// The link's counters now, or a fault if the private counters are borrowed.
    pub fn link_health(&self) -> Result<LinkHealth, ConsoleFault> {
        let received = self.received.lock(|received| {
            received
                .try_borrow()
                .map(|received| *received)
                .map_err(|_| ConsoleFault::CountersBorrowed { writing: false })
        })?;
        Ok(crate::base::link_health(received, self.sent()))
    }

    /// Publishes an answer of the base module for an image serving
    /// `image_keys` on `platform`.
    fn answer(
        &self,
        request: RequestIdentity,
        answer: Answer,
        image_keys: ImageKeySet<'_>,
        platform: &impl Platform,
    ) {
        let (session, id) = (request.session_id, request.request_id);
        // A lost answer is counted in the link's health; the host's request
        // then times out.
        let _counted = match answer {
            Answer::Hello(hello) => self.publish(session, id, &hello),
            Answer::ImageKeys(first) => self.publish(session, id, &image_keys.page(first)),
            Answer::Boot => self.publish(session, id, &platform.boot_evidence()),
            Answer::PostMortem(first) => {
                self.publish(session, id, &platform.post_mortem_checkpoints(first))
            }
            Answer::Link(link) => self.publish(session, id, &link),
            Answer::Rejected(reason) => self.publish(session, id, &Rejected(reason)),
        };
    }

    /// Serves the endpoint until the chip resets: writes queued messages and
    /// notes, and hands each request of the image to `serve`, which must
    /// not block; the base module's requests are answered here. A broken read
    /// contract or counter borrow returns a fault retaining the endpoint.
    pub async fn run<C: Requests, E: Read + Write>(
        &self,
        mut endpoint: E,
        mut intake: Intake<'_>,
        platform: &impl Platform,
        mut serve: impl FnMut(RequestIdentity, C),
    ) -> RunFault<E> {
        self.running.store(true, Ordering::Release);
        let image_keys = intake.image_keys();
        match Outbound::new(HELLO_SEQUENCE, 0, 0, &intake.hello()) {
            Ok(hello) => {
                let _written = self.write_message(&mut endpoint, &hello, platform).await;
            }
            Err(_) => self.lose_message(),
        }
        // What the early writer dropped before it gave up the peripheral: a
        // loss is never silent.
        let lost = P::take_lost();
        if lost != 0 {
            self.write_note(&mut endpoint, &Note::diag_lost(lost)).await;
        }
        let mut chunk = [0_u8; RX_CHUNK];
        loop {
            match select3(
                self.messages.receive(),
                endpoint.read(&mut chunk),
                self.notes.receive(),
            )
            .await
            {
                Either3::First(message) => {
                    self.write_queued(&mut endpoint, message, platform).await;
                }
                Either3::Second(Ok(length)) => {
                    let Some(bytes) = chunk.get(..length) else {
                        return RunFault {
                            cause: ConsoleFault::ReadLength {
                                length,
                                capacity: chunk.len(),
                            },
                            endpoint,
                        };
                    };
                    intake.receive_each::<C>(bytes, self.sent(), |request| match request {
                        Request::Answer(identity, answer) => {
                            self.answer(identity, answer, image_keys, platform)
                        }
                        Request::Serve(identity, own) => serve(identity, own),
                    });
                    let counters = intake.counters();
                    let updated = self.received.lock(|received| {
                        received
                            .try_borrow_mut()
                            .map(|mut received| *received = counters)
                    });
                    if updated.is_err() {
                        return RunFault {
                            cause: ConsoleFault::CountersBorrowed { writing: true },
                            endpoint,
                        };
                    }
                }
                Either3::Second(Err(_)) => {}
                Either3::Third(note) => self.write_note(&mut endpoint, &note).await,
            }
            for _ in 1..DRAIN_BATCH {
                if let Ok(message) = self.messages.try_receive() {
                    self.write_queued(&mut endpoint, message, platform).await;
                } else if let Ok(note) = self.notes.try_receive() {
                    self.write_note(&mut endpoint, &note).await;
                } else {
                    break;
                }
            }
            embassy_futures::yield_now().await;
        }
    }

    /// Numbers a message taken out of the queue, writes it, and on its
    /// write tells the waiters how many entries are written.
    async fn write_queued(
        &self,
        tx: &mut impl Write,
        mut message: Outbound,
        platform: &impl Platform,
    ) {
        message.message_sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let taken = self.taken.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if self.write_message(tx, &message, platform).await {
            self.written.publish_next(taken);
        }
    }

    /// Writes one message whole; whether it was.
    async fn write_message(
        &self,
        tx: &mut impl Write,
        message: &Outbound,
        platform: &impl Platform,
    ) -> bool {
        let mut encoder = self.encoder.lock().await;
        let Ok(frame) = encoder.encode_outbound(self.boot_id(), message) else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if write_whole(tx, frame).await.is_ok() && write_whole(tx, b"\r\n").await.is_ok() {
            self.sent_frames.fetch_add(1, Ordering::Relaxed);
            if message.key == oer_protocols_hil_protocol::base::BootEvidence::KEY {
                platform.boot_evidence_written();
            }
            true
        } else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Numbers and writes one note as a whole line.
    async fn write_note(&self, tx: &mut impl Write, note: &Note<'_>) {
        let mut line = NoteLine::new();
        line.encode(note);
        if write_whole(tx, line.framed()).await.is_err() {
            self.lost.write_errors.fetch_add(1, Ordering::Relaxed);
            self.lost.dropped.fetch_add(1, Ordering::Relaxed);
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
