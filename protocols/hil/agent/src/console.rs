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
//! [`Console::publish_reliably`] waits), and the one [`Console::run`] task
//! frames and writes messages and notes in queue order. A note is a typed
//! [`oer_diagnostics::Note`]; the console numbers it when it writes it, so
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
use oer_diagnostics::{MAX_NOTE_LINE, Note, emit_note};
use oer_hil_protocol::base::{ImageKeySet, LinkHealth, Rejected};
use oer_hil_protocol::{DecodeCounters, FrameEncoder, Message, Outbound, RequestIdentity};

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
    /// Writes `note` now. Returns `false` when the early writer no longer
    /// owns the peripheral, which the transport has taken.
    fn note(note: &Note<'_>) -> bool;

    /// The bytes the early writer dropped since the last call.
    fn take_lost() -> u32;
}

/// One console. `NOTES` bounds the queued notes and `MESSAGES` the queued
/// messages; `P` is the chip's early writer.
pub struct Console<const NOTES: usize, const MESSAGES: usize, P: Immediate> {
    immediate: PhantomData<P>,
    running: AtomicBool,
    boot_low: AtomicU32,
    boot_high: AtomicU32,
    sequence: AtomicU32,
    // Sequence reservation and queue insertion are one producer transaction:
    // reserving before an async send would let a later producer queue N+1
    // ahead of N, which makes an otherwise lossless stream fail closed.
    publishing: AsyncMutex<CriticalSectionRawMutex, ()>,
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
            publishing: AsyncMutex::new(()),
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
        self.written.publish_next(0);
    }

    pub fn boot_id(&self) -> u64 {
        u64::from(self.boot_low.load(Ordering::Acquire))
            | (u64::from(self.boot_high.load(Ordering::Acquire)) << 32)
    }

    /// Queues `body` without making the caller wait for the endpoint; a full
    /// queue drops it and counts the loss. Returns the message's sequence.
    // Keep the publication guard's wake in this frame: inlining its drop
    // into Intake loses the typed Waker scope needed by stack analysis.
    #[inline(never)]
    pub fn publish<M: Message>(&self, session_id: u64, request_id: u32, body: &M) -> Option<u32> {
        let Ok(_publishing) = self.publishing.try_lock() else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let queued = Outbound::new(sequence, session_id, request_id, body)
            .ok()
            .and_then(|message| self.messages.try_send(message).ok());
        if queued.is_none() {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(sequence)
    }

    /// Queues `body`, waiting for room: for control-plane boundaries a full
    /// telemetry queue must not erase. High-rate observations use
    /// [`Console::publish`], which never applies backpressure.
    pub async fn publish_reliably<M: Message>(
        &self,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> u32 {
        let _publishing = self.publishing.lock().await;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        match Outbound::new(sequence, session_id, request_id, body) {
            Ok(message) => self.messages.send(message).await,
            Err(_) => {
                self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            }
        }
        sequence
    }

    /// Waits until the message with `sequence` has been written to the
    /// endpoint.
    pub async fn written(&self, sequence: u32) -> Result<(), progress::ReceiverLimit> {
        self.written.wait_for(sequence).await
    }

    /// Writes `note` through the early writer while it owns the USB
    /// peripheral, and queues it for [`Console::run`] once the transport
    /// does, waiting for room instead of losing it. Only for reporting
    /// outside measured hot paths, which must not depend on USB progress.
    pub async fn note_reliably(&self, note: &Note<'static>) {
        if !self.running.load(Ordering::Acquire) && P::note(note) {
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
        if !self.running.load(Ordering::Acquire) && P::note(note) {
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
        match answer {
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
            Ok(hello) => self.write_message(&mut endpoint, &hello, platform).await,
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
                    self.write_message(&mut endpoint, &message, platform).await
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
                    self.write_message(&mut endpoint, &message, platform).await;
                } else if let Ok(note) = self.notes.try_receive() {
                    self.write_note(&mut endpoint, &note).await;
                } else {
                    break;
                }
            }
            embassy_futures::yield_now().await;
        }
    }

    async fn write_message(
        &self,
        tx: &mut impl Write,
        message: &Outbound,
        platform: &impl Platform,
    ) {
        let mut encoder = self.encoder.lock().await;
        let Ok(frame) = encoder.encode_outbound(self.boot_id(), message) else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if tx.write_all(frame).await.is_ok() && tx.write_all(b"\r\n").await.is_ok() {
            self.sent_frames.fetch_add(1, Ordering::Relaxed);
            if message.key == oer_hil_protocol::base::BootEvidence::KEY {
                platform.boot_evidence_written();
            }
            self.written
                .publish_next(message.message_sequence.wrapping_add(1));
        } else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Numbers and writes one note as a whole line.
    async fn write_note(&self, tx: &mut impl Write, note: &Note<'_>) {
        let mut line = NoteLine::new();
        line.encode(note);
        if tx.write_all(line.framed()).await.is_err() {
            self.lost.write_errors.fetch_add(1, Ordering::Relaxed);
            self.lost.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
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
