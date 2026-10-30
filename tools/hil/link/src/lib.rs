//! The host end of the HIL link: one board's serial line, the protocol's
//! frames on it, the boot they belong to and the request/response exchange.
//!
//! The link knows the framework's `base` module and nothing else: every
//! other message is a header, a key and a payload, which the caller decodes
//! as the type it expects. It stores nothing; the device's diagnostic lines
//! come back as text for the caller to print.
//!
//! A boot is named by the `boot_id` every frame carries. The boot's first
//! frame is its Hello; [`Link::boot`] also asks a running boot for it again
//! (`base::GetHello`), which answers with the same boot's Hello. A frame of
//! another boot means the device restarted, and every request of the earlier
//! boot ended with it.
//!
//! A request reserves the slot of its response before it is sent, so no
//! other message can displace the response. Every other message of the
//! running boot is kept in a bounded ring for a [`Cursor`], which tells
//! whether its interval lost one. Every queue is bounded: the host's frames
//! wait in [`OUTBOUND_CAPACITY`] places and a full queue refuses a request
//! before it leaves.
#![forbid(unsafe_code)]

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    DecodeCounters, Endpoint, Envelope, Frame, FrameEncoder, Key, Message,
    base::{GetHello, Hello, Rejected},
};
use state::State;
use zeroize::Zeroizing;

mod error;
mod notes;
mod state;
mod target;
mod transport;
mod worker;

pub use error::{CallError, LinkError, NotSent};
pub use state::{Completeness, EVENT_CAPACITY, MAX_PENDING, NOTES_CAPACITY, PAST_BOOTS};
pub use target::SerialLine;

/// Host frames waiting for the worker to write them.
pub const OUTBOUND_CAPACITY: usize = 64;

/// How often a waiting caller looks at cancellation.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// One device message: its header, its key and its payload.
#[derive(Clone)]
pub struct Received {
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub key: Key,
    payload: Vec<u8>,
}

impl std::fmt::Debug for Received {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Received")
            .field("boot_id", &self.boot_id)
            .field("message_sequence", &self.message_sequence)
            .field("session_id", &self.session_id)
            .field("request_id", &self.request_id)
            .field("key", &self.key)
            .field("payload_length", &self.payload.len())
            .finish()
    }
}

impl Drop for Received {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.payload);
    }
}

impl Received {
    fn from_frame(frame: &Frame<'_>) -> Self {
        Self {
            boot_id: frame.boot_id,
            message_sequence: frame.message_sequence,
            session_id: frame.session_id,
            request_id: frame.request_id,
            key: frame.key,
            payload: frame.payload.to_vec(),
        }
    }

    pub fn is<M: Message>(&self) -> bool {
        self.key == M::KEY
    }

    /// This message as `M`: `None` when its key is another type's, an error
    /// when its key is `M`'s but its payload does not decode as `M`.
    pub fn decode<M: Message>(&self) -> Option<Result<M, CallError>> {
        self.is::<M>().then(|| {
            postcard::from_bytes(&self.payload).map_err(|_| CallError::Payload { path: M::PATH })
        })
    }
}

/// The running boot, as its Hello describes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Boot {
    pub boot_id: u64,
    pub hello: Hello,
}

/// What the link observed of the wire.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Health {
    /// The host decoder's counters over the whole line.
    pub counters: DecodeCounters,
    /// Boots seen on the line.
    pub boots: u32,
    /// The running boot's counters.
    pub running: Option<BootStats>,
    /// The counters of the last [`PAST_BOOTS`] boots that ended, oldest
    /// first.
    pub past: Vec<BootStats>,
    /// Diagnostic lines displaced before anyone took them.
    pub notes_lost: u64,
}

/// One boot's message counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BootStats {
    pub boot_id: u64,
    /// Device messages missing from the boot's sequence.
    pub missing: u64,
    /// Kept messages a newer one displaced before anyone took them.
    pub displaced: u64,
    /// Kept messages nobody took: so far, or before the boot ended.
    pub unread: u64,
}

/// A position in one boot's messages: [`Link::take`] returns only messages
/// that arrived after it, and [`Link::completeness`] tells whether one of
/// them was lost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    boot_id: u64,
    start: u64,
}

impl Cursor {
    pub fn boot_id(&self) -> u64 {
        self.boot_id
    }
}

/// The device's diagnostic lines since the last [`Link::take_notes`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceNotes {
    /// Each line rendered as text.
    pub lines: Vec<String>,
    /// Lines displaced before this call took them.
    pub lost: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The link to one board: a worker thread owns its serial line.
pub struct Link {
    shared: Arc<Shared>,
    outbound: mpsc::SyncSender<Zeroizing<Vec<u8>>>,
    wake: Arc<mio::Waker>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    encoder: Mutex<FrameEncoder>,
    next_request: AtomicU32,
    next_sequence: AtomicU32,
}

impl Link {
    /// Start reading and writing `line`. A line the caller opened with a
    /// reset shows the boot from its first byte.
    pub fn start(line: Box<dyn SerialLine>) -> Result<Self, LinkError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (outbound, commands) = mpsc::sync_channel(OUTBOUND_CAPACITY);
        let poll = transport::events::EventPoll::new().map_err(|error| {
            LinkError::Transport(format!("cannot poll the serial line: {error}"))
        })?;
        let wake = poll.waker();
        let serial = worker::nonblocking(line)?;
        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("hil-link".to_owned())
                .spawn(move || worker::run(serial, poll, &shared, &stop, commands))
                .map_err(|error| LinkError::Transport(format!("cannot start the link: {error}")))?
        };
        Ok(Self {
            shared,
            outbound,
            wake,
            stop,
            worker: Some(worker),
            encoder: Mutex::new(FrameEncoder::new()),
            next_request: AtomicU32::new(1),
            next_sequence: AtomicU32::new(0),
        })
    }

    /// The running boot: its own Hello, or, when the link missed it, the
    /// answer to `base::GetHello`, asked once after `timeout / 2`.
    pub fn boot(&self, timeout: Duration) -> Result<Boot, LinkError> {
        let deadline = Instant::now() + timeout;
        let ask_at = Instant::now() + timeout / 2;
        let mut asked = false;
        let mut state = self.shared.lock();
        loop {
            if let Some(error) = state.failure() {
                return Err(error.clone());
            }
            if let Some(boot) = state.hello() {
                return Ok(boot);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(LinkError::NoHello);
            }
            if !asked && now >= ask_at {
                drop(state);
                // Discovery names no boot: any running boot answers it.
                let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
                self.send(0, request_id, GetHello).map_err(|error| {
                    LinkError::Transport(format!("GetHello was not sent: {error}"))
                })?;
                asked = true;
                state = self.shared.lock();
                continue;
            }
            state = self.wait(state, deadline.min(if asked { deadline } else { ask_at }))?;
        }
    }

    /// Send `request` to `boot` and wait up to `timeout` for its response.
    /// The response's slot is reserved before the request leaves; other
    /// messages of the same request (related events) stay for a cursor.
    pub fn call<E: Endpoint>(
        &self,
        boot: &Boot,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, CallError> {
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
        if !self
            .shared
            .lock()
            .expect(boot.boot_id, request_id, <E::Response as Message>::KEY)
        {
            return Err(CallError::NotSent(NotSent::PendingFull));
        }
        let result = self.exchange::<E>(boot, request_id, request, timeout);
        self.shared.lock().forget(request_id);
        result
    }

    fn exchange<E: Endpoint>(
        &self,
        boot: &Boot,
        request_id: u32,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, CallError> {
        self.send(boot.boot_id, request_id, request)?;
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(message) = state.response(request_id) {
                if let Some(rejected) = message.decode::<Rejected>() {
                    return Err(CallError::Rejected(rejected?.0));
                }
                return message
                    .decode::<E::Response>()
                    .unwrap_or(Err(CallError::Payload {
                        path: <E::Response as Message>::PATH,
                    }));
            }
            if let Some(error) = state.failure() {
                return Err(error.clone().into());
            }
            if let Some(boot_id) = state.boot_id()
                && boot_id != boot.boot_id
            {
                return Err(CallError::Rebooted { boot_id });
            }
            if Instant::now() >= deadline {
                return Err(CallError::Unconfirmed { request_id });
            }
            state = self.wait(state, deadline)?;
        }
    }

    /// A cursor over the messages of `boot` that arrive from now on. A
    /// cursor made before a request sees its related events, also those
    /// that come before its response.
    pub fn cursor(&self, boot: &Boot) -> Cursor {
        self.shared.lock().cursor(boot.boot_id)
    }

    /// The first kept message since `cursor` started that `matches`
    /// accepts, waiting up to `timeout`; `None` when none came. A cursor
    /// whose boot ended returns `Rebooted`.
    pub fn take(
        &self,
        cursor: &Cursor,
        matches: impl Fn(&Received) -> bool,
        timeout: Duration,
    ) -> Result<Option<Received>, CallError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(message) = state.take(cursor, &matches) {
                return Ok(Some(message));
            }
            if let Some(error) = state.failure() {
                return Err(error.clone().into());
            }
            if let Completeness::Ended { boot_id } = state.completeness(cursor) {
                return Err(CallError::Rebooted { boot_id });
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            state = self.wait(state, deadline)?;
        }
    }

    /// Whether every message of `cursor`'s interval arrived and was kept.
    pub fn completeness(&self, cursor: &Cursor) -> Completeness {
        self.shared.lock().completeness(cursor)
    }

    /// What the link observed of the wire so far.
    pub fn health(&self) -> Health {
        self.shared.lock().health()
    }

    /// The device's diagnostic lines since the last call, as text, and how
    /// many were lost.
    pub fn take_notes(&self) -> DeviceNotes {
        self.shared.lock().take_notes()
    }

    /// Queue `message` for the worker without waiting: an error means the
    /// frame did not leave the host.
    fn send<M: Message>(&self, boot_id: u64, request_id: u32, message: M) -> Result<(), CallError> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let envelope = Envelope::new(boot_id, sequence, 0, request_id, message);
        let frame = {
            let mut encoder = self
                .encoder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Zeroizing::new(
                encoder
                    .encode(&envelope)
                    .map_err(|_| CallError::NotSent(NotSent::Encode))?
                    .to_vec(),
            )
        };
        self.outbound.try_send(frame).map_err(|error| {
            CallError::NotSent(match error {
                mpsc::TrySendError::Full(_) => NotSent::QueueFull,
                mpsc::TrySendError::Disconnected(_) => NotSent::LinkEnded,
            })
        })?;
        // The frame is queued: from here on its outcome is unknown.
        self.wake
            .wake()
            .map_err(|error| LinkError::Transport(format!("cannot wake the link: {error}")))?;
        Ok(())
    }

    /// Wait for a change until `deadline`, looking at cancellation.
    fn wait<'a>(
        &self,
        state: std::sync::MutexGuard<'a, State>,
        deadline: Instant,
    ) -> Result<std::sync::MutexGuard<'a, State>, LinkError> {
        if oer_os_process::cancellation_requested() {
            return Err(LinkError::Cancelled);
        }
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(CANCEL_POLL);
        Ok(self
            .shared
            .changed
            .wait_timeout(state, timeout)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
