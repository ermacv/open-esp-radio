//! A capture process's output, read by a thread of its own into a bounded
//! channel, and the clock every wait of the capture goes through.
//!
//! The reader is controlled: it waits in `poll` on its source and on a stop
//! pipe it alone reads, never in a bare `read`. [`Stream::close`] drops the
//! channel's receiving end (a blocked send fails at once) and the stop
//! pipe's writing end (a waiting `poll` wakes at once), so the reader ends
//! whatever its source does; then the close waits for the reader's end
//! event through the clock and joins it. A reader is never detached: if the
//! end does not come by the deadline the handle stays in the stream, and
//! dropping the stream joins it.

use std::{
    io::{PipeReader, PipeWriter, Read},
    os::fd::{AsFd, BorrowedFd},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, poll};

use crate::ssh::Result;

/// What a wait for a message ended with.
#[derive(Debug, Eq, PartialEq)]
pub enum Wait<T> {
    Message(T),
    /// The deadline passed first.
    Timeout,
    /// Every sender is gone.
    Disconnected,
}

/// The time the capture's waits are bounded by.
pub trait Clock {
    type Instant: Copy + Ord;
    fn now(&self) -> Self::Instant;
    fn after(&self, from: Self::Instant, by: Duration) -> Self::Instant;
    /// The time left until `until`, for a wait only the host's process
    /// API can bound.
    fn remaining(&self, until: Self::Instant) -> Duration;
    /// The next message of `channel`, waiting until `until`.
    fn recv<T>(&self, channel: &mpsc::Receiver<T>, until: Self::Instant) -> Wait<T>;
}

/// The host's monotonic time.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostClock;

impl Clock for HostClock {
    type Instant = Instant;

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn after(&self, from: Instant, by: Duration) -> Instant {
        from.checked_add(by).unwrap_or(from)
    }

    fn remaining(&self, until: Instant) -> Duration {
        until.saturating_duration_since(Instant::now())
    }

    fn recv<T>(&self, channel: &mpsc::Receiver<T>, until: Instant) -> Wait<T> {
        match channel.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(message) => Wait::Message(message),
            Err(mpsc::RecvTimeoutError::Timeout) => Wait::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => Wait::Disconnected,
        }
    }
}

/// What the reader is doing, for an observer that drives time by events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderEvent {
    /// It is about to wait in `poll`.
    Waiting,
    /// `poll` returned.
    Woke,
    /// It is about to hand a chunk to the channel; it blocks while the
    /// channel is full.
    Sending,
    /// It handed a chunk to the channel.
    Delivered,
    /// The stream's owner took a chunk from the channel.
    Taken,
    /// The stream was told to stop.
    Stopping,
    /// The reader's thread reported its end.
    Ended,
}

/// An observer of the reader's events; `()` observes nothing.
pub trait Observer: Clone + Send + 'static {
    fn event(&self, event: ReaderEvent);
    /// The two descriptors the reader waits on, before it starts: its
    /// source and its stop pipe.
    fn watch(&self, _source: BorrowedFd<'_>, _stop: BorrowedFd<'_>) {}
}

impl Observer for () {
    fn event(&self, _: ReaderEvent) {}
}

/// The next look at a stream.
#[derive(Debug, Eq, PartialEq)]
pub enum Next {
    Bytes(Vec<u8>),
    /// Nothing until the deadline.
    Idle,
    /// The stream ended.
    Ended,
}

/// The capacity of a stream's channel, in chunks.
pub const STREAM_DEPTH: usize = 64;
const CHUNK: usize = 64 * 1024;

/// A reader's last word.
type End = Result<u64>;

pub struct Stream {
    chunks: Option<mpsc::Receiver<Vec<u8>>>,
    ended: mpsc::Receiver<End>,
    stop: Option<PipeWriter>,
    reader: Option<JoinHandle<()>>,
    observer: Box<dyn Fn(ReaderEvent) + Send>,
}

/// Wait for the source or the stop pipe; whether the source may be read.
fn ready(source: &impl AsFd, stop: &PipeReader) -> Result<bool> {
    let mut fds = [
        PollFd::new(source, PollFlags::IN),
        PollFd::new(stop, PollFlags::IN),
    ];
    loop {
        match poll(&mut fds, None) {
            Ok(_) => break,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(format!("the stream's poll failed: {error}")),
        }
    }
    let [source, stop] = &fds;
    if !stop.revents().is_empty() {
        return Ok(false);
    }
    Ok(!source.revents().is_empty())
}

fn read_until_stopped<S: Read + AsFd>(
    source: &mut S,
    stop: &PipeReader,
    send: &mpsc::SyncSender<Vec<u8>>,
    observer: &impl Observer,
) -> End {
    let mut buffer = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        observer.event(ReaderEvent::Waiting);
        let readable = ready(source, stop);
        observer.event(ReaderEvent::Woke);
        if !readable? {
            return Err(format!("the stream was stopped ({total} bytes read)"));
        }
        match source.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(count) => {
                let chunk = buffer.get(..count).unwrap_or_default().to_vec();
                total = total.saturating_add(count as u64);
                observer.event(ReaderEvent::Sending);
                if send.send(chunk).is_err() {
                    return Err(format!(
                        "the stream was closed with its output still flowing ({total} bytes read)"
                    ));
                }
                observer.event(ReaderEvent::Delivered);
            }
            Err(error) => return Err(format!("the stream's read failed: {error}")),
        }
    }
}

impl Stream {
    /// Read `source` until its end, an error, a closed channel or a stop;
    /// `observer` sees the reader's events.
    ///
    /// # Errors
    /// The stop pipe could not be made.
    pub fn spawn<S: Read + AsFd + Send + 'static>(
        mut source: S,
        observer: impl Observer,
    ) -> Result<Self> {
        let (stop_reader, stop) =
            std::io::pipe().map_err(|error| format!("the stream's stop pipe: {error}"))?;
        let (send, chunks) = mpsc::sync_channel(STREAM_DEPTH);
        let (report, ended) = mpsc::sync_channel(1);
        observer.watch(source.as_fd(), stop_reader.as_fd());
        let inside = observer.clone();
        let reader = std::thread::spawn(move || {
            let end = read_until_stopped(&mut source, &stop_reader, &send, &inside);
            drop(source);
            drop(send);
            let _ = report.send(end);
            inside.event(ReaderEvent::Ended);
        });
        Ok(Self {
            chunks: Some(chunks),
            ended,
            stop: Some(stop),
            reader: Some(reader),
            observer: Box::new(move |event| observer.event(event)),
        })
    }

    /// The next chunk, waiting until `until` on `clock`.
    pub fn next<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Next {
        let Some(chunks) = &self.chunks else {
            return Next::Ended;
        };
        match clock.recv(chunks, until) {
            Wait::Message(chunk) => {
                (self.observer)(ReaderEvent::Taken);
                Next::Bytes(chunk)
            }
            Wait::Timeout => Next::Idle,
            Wait::Disconnected => Next::Ended,
        }
    }

    /// Stop the reader, wait for its end until `until` and join it: what it
    /// read in all, or why it ended otherwise.
    ///
    /// # Errors
    /// The reader's own error, a panic of its thread, or no end by
    /// `until`: then its handle stays here, and dropping the stream joins it.
    pub fn close<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<u64> {
        (self.observer)(ReaderEvent::Stopping);
        drop(self.chunks.take());
        drop(self.stop.take());
        match clock.recv(&self.ended, until) {
            Wait::Message(end) => {
                let reader = self.reader.take().ok_or("the stream was already closed")?;
                reader
                    .join()
                    .map_err(|_| "the stream's reader panicked".to_owned())?;
                end
            }
            Wait::Disconnected => {
                let joined = self.reader.take().map(JoinHandle::join);
                match joined {
                    Some(Err(_)) => Err("the stream's reader panicked".into()),
                    Some(Ok(())) => Err("the stream's reader ended without a word".into()),
                    None => Err("the stream was already closed".into()),
                }
            }
            Wait::Timeout => Err("the stream's reader did not end by the deadline".into()),
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Stopped by construction: the channel and the stop pipe are gone.
        drop(self.chunks.take());
        drop(self.stop.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
