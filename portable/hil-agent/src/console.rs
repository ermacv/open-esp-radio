//! The target's one HIL console.
//!
//! Every chip and image uses this console. The chip supplies its byte
//! transport (the halves of an `embedded-io-async` endpoint) and a way to
//! write one line immediately; the image supplies its [`crate::base::Platform`] and
//! serves its own requests. The console owns everything between: the ordered
//! queue of outgoing messages, the bounded text log, the request intake with
//! the base module, and the link counters.
//!
//! Producers never touch the endpoint. They serialize their message into
//! the queue ([`Console::publish`] drops it when the queue is full,
//! [`Console::publish_reliably`] waits), and the one [`Console::run`] task
//! frames and writes messages and text lines in queue order.

pub mod progress;
mod text;
pub mod writer;

pub use text::TextBuffer;

use core::cell::RefCell;
use core::ffi::CStr;
use core::fmt::Arguments;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embedded_io_async::{Read, Write};
use oer_hil_protocol::base::{ImageKeySet, LinkHealth, Rejected};
use oer_hil_protocol::{DecodeCounters, FrameEncoder, Message, Outbound, RequestIdentity};

use crate::base::{Answer, Intake, Platform, Request, Requests, Sent};

/// Text lines written back to back before the task looks at the queue again.
const DRAIN_BATCH: usize = 4;
/// The boot's first message: the host's handshake expects Hello here.
const HELLO_SEQUENCE: u32 = 0;
/// Bytes read from the endpoint at once.
const RX_CHUNK: usize = 128;

/// One console. `LINE` bounds one text line, `LINES` the queued lines and
/// `MESSAGES` the queued messages.
pub struct Console<const LINE: usize, const LINES: usize, const MESSAGES: usize> {
    /// Writes one line synchronously, for boot, panic and the time before
    /// [`Console::run`] starts.
    immediate: fn(&CStr),
    writer: writer::Writer,
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
    lines: Channel<CriticalSectionRawMutex, TextBuffer<LINE>, LINES>,
    written: progress::SerializedEvents,
    sent_frames: AtomicU32,
    dropped_messages: AtomicU32,
    received: Mutex<CriticalSectionRawMutex, RefCell<DecodeCounters>>,
    text: TextLoss,
}

/// What the text log lost.
struct TextLoss {
    dropped: AtomicU32,
    queue_full: AtomicU32,
    writer_busy: AtomicU32,
    write_errors: AtomicU32,
    truncated: AtomicU32,
    first_queue_loss: Mutex<CriticalSectionRawMutex, RefCell<Option<TextBuffer<160>>>>,
}

impl<const LINE: usize, const LINES: usize, const MESSAGES: usize> Console<LINE, LINES, MESSAGES> {
    pub const fn new(immediate: fn(&CStr)) -> Self {
        Self {
            immediate,
            writer: writer::Writer::new(),
            running: AtomicBool::new(false),
            boot_low: AtomicU32::new(0),
            boot_high: AtomicU32::new(0),
            sequence: AtomicU32::new(0),
            publishing: AsyncMutex::new(()),
            encoder: AsyncMutex::new(FrameEncoder::new()),
            messages: Channel::new(),
            lines: Channel::new(),
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
            text: TextLoss {
                dropped: AtomicU32::new(0),
                queue_full: AtomicU32::new(0),
                writer_busy: AtomicU32::new(0),
                write_errors: AtomicU32::new(0),
                truncated: AtomicU32::new(0),
                first_queue_loss: Mutex::new(RefCell::new(None)),
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
    pub async fn written(&self, sequence: u32) {
        self.written.wait_for(sequence).await;
    }

    /// Queues one text line; before [`Console::run`] starts, writes it
    /// immediately.
    pub fn line(&self, args: Arguments<'_>) {
        if !self.running.load(Ordering::Acquire) {
            self.line_immediately(args);
            return;
        }
        // Under sustained pressure, skip even the formatting of a line that
        // cannot enter the queue; `try_send` stays the authoritative check.
        if self.lines.is_full() {
            self.lose_queued_line(args);
            return;
        }
        let line = self.format(args);
        if self.lines.try_send(line).is_err() {
            self.lose_queued_line(args);
        }
    }

    /// Queues one text line, waiting for room instead of losing it; before
    /// [`Console::run`] starts, writes it immediately. Only for reporting
    /// outside measured hot paths, which must not depend on USB progress.
    pub async fn line_reliably(&self, args: Arguments<'_>) {
        if !self.running.load(Ordering::Acquire) {
            self.line_immediately(args);
            return;
        }
        // Wait for room before formatting, so the caller's future never
        // holds a whole line across the await.
        loop {
            core::future::poll_fn(|cx| self.lines.poll_ready_to_send(cx)).await;
            if self.lines.try_send(self.format(args)).is_ok() {
                return;
            }
        }
    }

    /// Counts a message its producer could not build.
    pub fn lose_message(&self) {
        self.dropped_messages.fetch_add(1, Ordering::Relaxed);
    }

    /// Writes one line now, for boot, panic and last-resort diagnostics. It
    /// never interleaves with a frame: while the task writes, the line is
    /// dropped and counted.
    pub fn line_immediately(&self, args: Arguments<'_>) {
        let line = self.format(args);
        let Some(_writing) = self.writer.try_acquire() else {
            self.text.writer_busy.fetch_add(1, Ordering::Relaxed);
            self.text.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        (self.immediate)(line.as_c_str());
    }

    /// Text lines lost to a full queue, a busy writer or a failed write.
    pub fn dropped_lines(&self) -> u32 {
        self.text.dropped.load(Ordering::Relaxed)
    }

    /// Text lines cut to fit `LINE`.
    pub fn truncated_lines(&self) -> u32 {
        self.text.truncated.load(Ordering::Relaxed)
    }

    fn sent(&self) -> Sent {
        Sent {
            frames: self.sent_frames.load(Ordering::Acquire),
            dropped: self.dropped_messages.load(Ordering::Acquire),
            text_dropped: self.dropped_lines(),
            text_truncated: self.truncated_lines(),
        }
    }

    /// The link's counters now, for evidence that records them.
    pub fn link_health(&self) -> LinkHealth {
        let received = self.received.lock(|received| *received.borrow());
        crate::base::link_health(received, self.sent())
    }

    fn format(&self, args: Arguments<'_>) -> TextBuffer<LINE> {
        let line = TextBuffer::format(args);
        if line.was_truncated() {
            self.text.truncated.fetch_add(1, Ordering::Relaxed);
        }
        line
    }

    fn lose_queued_line(&self, args: Arguments<'_>) {
        if self.text.queue_full.fetch_add(1, Ordering::Relaxed) == 0 {
            let first = TextBuffer::format(args);
            self.text
                .first_queue_loss
                .lock(|slot| *slot.borrow_mut() = Some(first));
        }
        self.text.dropped.fetch_add(1, Ordering::Relaxed);
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
    /// lines, and hands each request of the image to `serve`, which must
    /// not block; the base module's requests are answered here.
    pub async fn run<C: Requests>(
        &self,
        mut rx: impl Read,
        mut tx: impl Write,
        mut intake: Intake<'_>,
        platform: &impl Platform,
        mut serve: impl FnMut(RequestIdentity, C),
    ) -> ! {
        self.running.store(true, Ordering::Release);
        let image_keys = intake.image_keys();
        match Outbound::new(HELLO_SEQUENCE, 0, 0, &intake.hello()) {
            Ok(hello) => self.write_message(&mut tx, &hello).await,
            Err(_) => self.lose_message(),
        }
        let mut chunk = [0_u8; RX_CHUNK];
        let mut reported = (0, 0);
        loop {
            match select3(
                self.messages.receive(),
                rx.read(&mut chunk),
                self.lines.receive(),
            )
            .await
            {
                Either3::First(message) => self.write_message(&mut tx, &message).await,
                Either3::Second(Ok(length)) => {
                    intake.receive_each::<C>(
                        &chunk[..length],
                        self.sent(),
                        |request| match request {
                            Request::Answer(identity, answer) => {
                                self.answer(identity, answer, image_keys, platform)
                            }
                            Request::Serve(identity, own) => serve(identity, own),
                        },
                    );
                    let counters = intake.counters();
                    self.received
                        .lock(|received| *received.borrow_mut() = counters);
                }
                Either3::Second(Err(_)) => {}
                Either3::Third(line) => self.write_line(&mut tx, &line).await,
            }
            for _ in 1..DRAIN_BATCH {
                if let Ok(message) = self.messages.try_receive() {
                    self.write_message(&mut tx, &message).await;
                } else if let Ok(line) = self.lines.try_receive() {
                    self.write_line(&mut tx, &line).await;
                } else {
                    break;
                }
            }
            self.report_text_loss(&mut tx, &mut reported).await;
            embassy_futures::yield_now().await;
        }
    }

    async fn write_message(&self, tx: &mut impl Write, message: &Outbound) {
        let mut encoder = self.encoder.lock().await;
        let Ok(frame) = encoder.encode_outbound(self.boot_id(), message) else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let _writing = self.writer.acquire_async().await;
        if tx.write_all(frame).await.is_ok() && tx.write_all(b"\r\n").await.is_ok() {
            self.sent_frames.fetch_add(1, Ordering::Relaxed);
            self.written
                .publish_next(message.message_sequence.wrapping_add(1));
        } else {
            self.dropped_messages.fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn write_line(&self, tx: &mut impl Write, line: &TextBuffer<LINE>) {
        let _writing = self.writer.acquire_async().await;
        if tx.write_all(line.as_bytes()).await.is_err() || tx.write_all(b"\r\n").await.is_err() {
            self.text.write_errors.fetch_add(1, Ordering::Relaxed);
            self.text.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes one line whenever the text log lost or cut lines since the
    /// last report.
    async fn report_text_loss(&self, tx: &mut impl Write, reported: &mut (u32, u32)) {
        let now = (self.dropped_lines(), self.truncated_lines());
        if now == *reported {
            return;
        }
        let first = self.text.first_queue_loss.lock(|first| *first.borrow());
        let first = first
            .as_ref()
            .and_then(|line| core::str::from_utf8(line.as_bytes()).ok())
            .unwrap_or("");
        let line = self.format(format_args!(
            "[WARN logger] dropped_total={} truncated_total={} queue_full={} writer_busy={} write_errors={} first_queue_drop={first:?}",
            now.0,
            now.1,
            self.text.queue_full.load(Ordering::Relaxed),
            self.text.writer_busy.load(Ordering::Relaxed),
            self.text.write_errors.load(Ordering::Relaxed),
        ));
        self.write_line(tx, &line).await;
        *reported = now;
    }
}

/// The `log` facade over a console: install it with [`log::set_logger`].
pub struct Logger<const LINE: usize, const LINES: usize, const MESSAGES: usize>(
    pub &'static Console<LINE, LINES, MESSAGES>,
);

impl<const LINE: usize, const LINES: usize, const MESSAGES: usize> log::Log
    for Logger<LINE, LINES, MESSAGES>
{
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.line(format_args!(
                "[{} {}] {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests;
