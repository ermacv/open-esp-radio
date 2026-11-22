//! The target's one HIL console.
//!
//! Every chip and image uses this console. The chip supplies its byte
//! transport (one duplex `embedded-io-async` endpoint) and the early writer
//! of its USB FIFO for the time before the transport owns the peripheral;
//! the image supplies its [`crate::base::Platform`] and serves its own
//! requests. The console's state is its one writer's, [`run`], and what the
//! producers share with it are lock-free FIFOs and atomics: no lock, no
//! critical section.
//!
//! - [`Lanes`]: one FIFO of outgoing messages a producing context, the
//!   image's, split once into a [`Producer`] a context and the writer's
//!   [`Drain`]. A producing context is a level of the console's hart: no
//!   producer runs on another hart, since no waker crosses harts. A
//!   producer's handle is its context's alone: it is not `Clone`, and it
//!   publishes through `&mut`.
//! - [`Writer`]: what only the writer touches (the encoder, the sequence it
//!   numbers messages with, its counts, and the answers of the base module
//!   it gives itself), placed once by the image and lent to [`run`].
//!
//! The writer numbers each message as it takes it out of a FIFO, so the
//! numbers follow the written order by construction. It writes Hello first,
//! then, at every pass, its own answers, then each lane in its declared
//! order, at most [`DRAIN_BATCH`] messages a lane. A producer that must know
//! its message written holds its place in its own lane ([`Entered`]) and
//! waits for the writer's count of that lane ([`Producer::written`]): one
//! producer a lane, so the place is exact, and the waiter borrows its
//! producer, so a lane has one waiter at a time and the wait cannot refuse.
//!
//! What is in a lane at once is bounded by the host, not by a type: the
//! host drives one request at a time per session and keeps at most one
//! posted beside it, and an image answers each once. A note is a typed
//! [`oer_portable_diagnostics::Note`]; the console writes one only for the
//! early writer's losses, numbered when written. The console formats no
//! text.

use core::future::poll_fn;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Poll;

use embassy_futures::select::{Either, select, select_array};
use embedded_io_async::{Read, Write};
use oer_portable_diagnostics::{MAX_NOTE_LINE, Note, emit_note};
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_portable_sync::PriorityWaker;
use oer_portable_waker::AtomicWaker;
use oer_protocols_hil_protocol::base::{ImageKeySet, Rejected};
use oer_protocols_hil_protocol::{FrameEncoder, Message, Outbound, RequestIdentity};

use crate::base::{Answer, Intake, Platform, Request, Requests, Sent};

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

/// Messages written back to back from one lane before the writer looks at
/// the next.
pub const DRAIN_BATCH: usize = 4;
/// The writer's own answers held at once: the base module's answers to the
/// requests of one read, and the busy rejections among them.
pub const OWN: usize = 8;
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

/// A message's place in its lane, for a wait until it is written
/// ([`Producer::written`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entered(u32);

impl Entered {
    /// The place, for a record that keeps it.
    pub const fn place(self) -> u32 {
        self.0
    }
}

/// A message lost before its lane: it could not be built, or the lane was
/// full. Counted in the link's health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dropped;

/// A request the image could not take now: the writer answers it
/// `Rejected(Busy)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

/// A lane's FIFO: the writer, on the console's level, the lowest that
/// produces, is woken by an ordinary waker when a message is entered (a
/// wake downward); a producer, on that level or above it on the same hart,
/// waits for room behind a [`PriorityWaker`] (a wake upward).
type Queue<const DEPTH: usize> = WakingSpscQueue<Outbound, DEPTH, AtomicWaker, PriorityWaker>;

/// One producing context's FIFO and what the writer tells it.
struct Lane<const DEPTH: usize> {
    queue: Queue<DEPTH>,
    /// The lane's messages lost before it, by its producer.
    dropped: AtomicU32,
    /// The lane's entries the writer wrote whole.
    written: AtomicU32,
    /// The lane's one waiter for a write ([`Producer::written`]): woken by
    /// the writer, below or beside it, so a [`PriorityWaker`].
    waiter: PriorityWaker,
}

/// What a lane's two ends share besides its FIFO.
#[derive(Clone, Copy)]
struct Shared<'a> {
    dropped: &'a AtomicU32,
    written: &'a AtomicU32,
    waiter: &'a PriorityWaker,
}

/// The image's `K` lanes of `DEPTH` messages each, split once.
pub struct Lanes<const K: usize, const DEPTH: usize> {
    lanes: [Lane<DEPTH>; K],
}

impl<const K: usize, const DEPTH: usize> Default for Lanes<K, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const K: usize, const DEPTH: usize> Lanes<K, DEPTH> {
    pub const fn new() -> Self {
        Self {
            lanes: [const {
                Lane {
                    // The constructor of a queue with named notifies; both
                    // ends are on the console's hart.
                    queue: WakingSpscQueue::across_harts(AtomicWaker::new(), PriorityWaker::new()),
                    dropped: AtomicU32::new(0),
                    written: AtomicU32::new(0),
                    waiter: PriorityWaker::new(),
                }
            }; K],
        }
    }

    /// Each lane's two ends, in the lanes' order: the producer its context
    /// holds and the drain the writer holds.
    pub fn split(&mut self) -> [Ends<'_, DEPTH>; K] {
        self.lanes.each_mut().map(|lane| {
            let Lane {
                queue,
                dropped,
                written,
                waiter,
            } = lane;
            let (sender, receiver) = queue.split();
            let shared = Shared {
                dropped,
                written,
                waiter,
            };
            Ends {
                producer: Producer {
                    sender,
                    shared,
                    entered: 0,
                },
                drain: LaneDrain {
                    receiver,
                    shared,
                    taken: 0,
                },
            }
        })
    }
}

/// One lane's two ends.
pub struct Ends<'a, const DEPTH: usize> {
    pub producer: Producer<'a, DEPTH>,
    pub drain: LaneDrain<'a, DEPTH>,
}

/// A producing context's handle on its lane. Its context's alone: not
/// `Clone`, publishing through `&mut`. A producing context is a level of
/// the console's hart, and the producer is moved there once.
pub struct Producer<'a, const DEPTH: usize> {
    sender: WakingSpscSender<'a, Outbound, DEPTH, AtomicWaker, PriorityWaker>,
    shared: Shared<'a>,
    /// The lane's entries so far: its producer's alone, so exact.
    entered: u32,
}

impl<const DEPTH: usize> Producer<'_, DEPTH> {
    /// Queues `body` without waiting; a full lane drops it and counts the
    /// loss.
    ///
    /// # Errors
    /// [`Dropped`] when the message could not be built or the lane was
    /// full: counted in the link's health.
    pub fn publish<M: Message>(
        &mut self,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> Result<(), Dropped> {
        let queued = Outbound::new(0, session_id, request_id, body)
            .ok()
            .and_then(|message| self.sender.try_send(message).ok());
        match queued {
            Some(()) => {
                let _place = self.enter();
                Ok(())
            }
            None => Err(self.drop_message()),
        }
    }

    /// Queues `body`, waiting for room in its lane: for control-plane
    /// boundaries a full lane must not erase. Its place in the lane is the
    /// message's own, for [`Self::written`].
    ///
    /// # Errors
    /// [`Dropped`] when the message could not be built: counted, nothing
    /// queued.
    pub async fn publish_reliably<M: Message>(
        &mut self,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> Result<Entered, Dropped> {
        let Ok(message) = Outbound::new(0, session_id, request_id, body) else {
            return Err(self.drop_message());
        };
        self.sender.ready().await.send(message);
        Ok(self.enter())
    }

    /// Waits until the writer has written its lane's messages up to
    /// `entered`: that message written whole. A lane has one waiter at a
    /// time, its producer's borrow, so the wait has a place and no refusal.
    pub async fn written(&mut self, entered: Entered) {
        let target = entered.0.wrapping_add(1);
        let shared = self.shared;
        poll_fn(|cx| {
            shared.waiter.register(cx.waker());
            let written = shared.written.load(Ordering::Acquire);
            // Places are exact within the lane; within half the counter's
            // range the order of two of them is unambiguous.
            if written.wrapping_sub(target) < (1 << 31) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    fn enter(&mut self) -> Entered {
        let place = self.entered;
        self.entered = place.wrapping_add(1);
        Entered(place)
    }

    fn drop_message(&self) -> Dropped {
        self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        Dropped
    }
}

/// The writer's end of one lane.
pub struct LaneDrain<'a, const DEPTH: usize> {
    receiver: WakingSpscReceiver<'a, Outbound, DEPTH, AtomicWaker, PriorityWaker>,
    shared: Shared<'a>,
    /// The lane's entries the writer took out.
    taken: u32,
}

/// What only the writer touches: the encoder, the sequence it numbers with,
/// its counts and its own answers. The image places it once and lends it to
/// [`run`].
pub struct Writer {
    encoder: FrameEncoder,
    sequence: u32,
    sent_frames: u32,
    /// Messages the writer lost: not encoded, not written whole, or its own
    /// answers past [`OWN`].
    dropped: u32,
    /// The writer's notes not written whole.
    notes_dropped: u32,
    own: heapless::Deque<Outbound, OWN>,
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
            own: heapless::Deque::new(),
        }
    }

    /// The counts the link's health reports: the writer's and every lane's
    /// losses.
    fn sent<const K: usize, const DEPTH: usize>(&self, lanes: &[LaneDrain<'_, DEPTH>; K]) -> Sent {
        let dropped = lanes
            .iter()
            .map(|lane| lane.shared.dropped.load(Ordering::Relaxed))
            .fold(self.dropped, u32::wrapping_add);
        Sent {
            frames: self.sent_frames,
            dropped,
            notes_dropped: self.notes_dropped,
        }
    }

    /// Queues one of the writer's own answers, or counts its loss.
    fn own<M: Message>(&mut self, request: RequestIdentity, body: &M) {
        let queued = Outbound::new(0, request.session_id, request.request_id, body)
            .ok()
            .and_then(|message| self.own.push_back(message).ok());
        if queued.is_none() {
            self.dropped = self.dropped.wrapping_add(1);
        }
    }

    /// Queues the base module's answer to `request` for an image serving
    /// `image_keys` on `platform`.
    fn answer(
        &mut self,
        request: RequestIdentity,
        answer: Answer,
        image_keys: ImageKeySet<'_>,
        platform: &impl Platform,
    ) {
        match answer {
            Answer::Hello(hello) => self.own(request, &hello),
            Answer::ImageKeys(first) => self.own(request, &image_keys.page(first)),
            Answer::Boot => self.own(request, &platform.boot_evidence()),
            Answer::PostMortem(first) => {
                self.own(request, &platform.post_mortem_checkpoints(first));
            }
            Answer::Link(link) => self.own(request, &link),
            Answer::Rejected(reason) => self.own(request, &Rejected(reason)),
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
            self.dropped = self.dropped.wrapping_add(1);
            return false;
        };
        if write_whole(tx, frame).await.is_ok() && write_whole(tx, b"\r\n").await.is_ok() {
            self.sent_frames = self.sent_frames.wrapping_add(1);
            if message.key == oer_protocols_hil_protocol::base::BootEvidence::KEY {
                platform.boot_evidence_written();
            }
            true
        } else {
            self.dropped = self.dropped.wrapping_add(1);
            false
        }
    }

    /// Writes one lane's message and, on its write, tells the lane's waiter
    /// how many of its entries are written.
    async fn write_lane<const DEPTH: usize>(
        &mut self,
        tx: &mut impl Write,
        boot: u64,
        lane: &mut LaneDrain<'_, DEPTH>,
        message: Outbound,
        platform: &impl Platform,
    ) {
        lane.taken = lane.taken.wrapping_add(1);
        if self.write(tx, boot, message, platform).await {
            lane.shared.written.store(lane.taken, Ordering::Release);
            lane.shared.waiter.wake();
        }
    }

    /// Numbers and writes one note as a whole line.
    async fn write_note(&mut self, tx: &mut impl Write, note: &Note<'_>) {
        let mut line = NoteLine::new();
        line.encode(note);
        if write_whole(tx, line.framed()).await.is_err() {
            self.notes_dropped = self.notes_dropped.wrapping_add(1);
        }
    }
}

/// Serves the endpoint until the chip resets, for the boot `boot`: writes
/// Hello, then at every pass the writer's own answers and each lane's
/// messages in the lanes' order, and hands each request of the image to
/// `serve`, which must not block; a [`Busy`] it answers is the writer's
/// `Rejected(Busy)`. The base module's requests are answered here. A broken
/// read contract returns a fault retaining the endpoint.
pub async fn run<P: Immediate, C: Requests, E: Read + Write, const K: usize, const DEPTH: usize>(
    writer: &mut Writer,
    lanes: &mut [LaneDrain<'_, DEPTH>; K],
    boot: u64,
    mut endpoint: E,
    mut intake: Intake<'_>,
    platform: &impl Platform,
    mut serve: impl FnMut(RequestIdentity, C) -> Result<(), Busy>,
) -> RunFault<E> {
    let image_keys = intake.image_keys();
    match Outbound::new(HELLO_SEQUENCE, 0, 0, &intake.hello()) {
        Ok(hello) => {
            let _written = writer.write(&mut endpoint, boot, hello, platform).await;
        }
        Err(_) => writer.dropped = writer.dropped.wrapping_add(1),
    }
    // What the early writer dropped before it gave up the peripheral: a
    // loss is never silent.
    let lost = P::take_lost();
    if lost != 0 {
        writer
            .write_note(&mut endpoint, &Note::diag_lost(lost))
            .await;
    }
    let mut chunk = [0_u8; RX_CHUNK];
    loop {
        let read = {
            let ready = select_array(lanes.each_mut().map(|lane| lane.receiver.ready()));
            select(ready, endpoint.read(&mut chunk)).await
        };
        match read {
            Either::First(_) | Either::Second(Err(_)) => {}
            Either::Second(Ok(length)) => {
                let Some(bytes) = chunk.get(..length) else {
                    return RunFault {
                        cause: ConsoleFault::ReadLength {
                            length,
                            capacity: chunk.len(),
                        },
                        endpoint,
                    };
                };
                let sent = writer.sent(lanes);
                intake.receive_each::<C>(bytes, sent, |request| match request {
                    Request::Answer(identity, answer) => {
                        writer.answer(identity, answer, image_keys, platform);
                    }
                    Request::Serve(identity, own) => {
                        if serve(identity, own).is_err() {
                            writer.own(
                                identity,
                                &Rejected(oer_protocols_hil_protocol::base::RejectReason::Busy),
                            );
                        }
                    }
                });
            }
        }
        while let Some(message) = writer.own.pop_front() {
            let _written = writer.write(&mut endpoint, boot, message, platform).await;
        }
        for lane in lanes.iter_mut() {
            for _ in 0..DRAIN_BATCH {
                let Some(message) = lane.receiver.try_receive() else {
                    break;
                };
                writer
                    .write_lane(&mut endpoint, boot, lane, message, platform)
                    .await;
            }
        }
        embassy_futures::yield_now().await;
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
