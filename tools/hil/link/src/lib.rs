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
#![forbid(unsafe_code)]

use std::{
    collections::VecDeque,
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
use zeroize::Zeroizing;

mod error;
mod notes;
mod target;
mod transport;
mod worker;

pub use error::{CallError, LinkError};
pub use target::SerialLine;

/// Device messages the link keeps until a caller takes them; one more is
/// counted in [`Health::dropped`] instead.
pub const RECEIVED_CAPACITY: usize = 4096;

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Health {
    /// The host decoder's counters over the whole line.
    pub counters: DecodeCounters,
    /// Device messages missing from a boot's sequence numbers.
    pub missing: u32,
    /// Device messages the host dropped because nobody took them.
    pub dropped: u32,
    /// Boots seen on the line.
    pub boots: u32,
}

#[derive(Default)]
struct State {
    received: VecDeque<Received>,
    /// The boot of the newest frame and the sequence number its next one
    /// carries.
    boot: Option<(u64, u32)>,
    /// The newest boot's Hello, once seen.
    hello: Option<Boot>,
    health: Health,
    notes: Vec<String>,
    failure: Option<LinkError>,
}

impl State {
    fn fail(&mut self, error: LinkError) {
        self.failure.get_or_insert(error);
    }

    /// Takes `message` into the boot and sequence accounting.
    fn receive(&mut self, message: Received) {
        if message.boot_id == 0 {
            self.fail(LinkError::Protocol(
                "the device sent the reserved boot identity 0".to_owned(),
            ));
            return;
        }
        match self.boot {
            Some((boot_id, next)) if boot_id == message.boot_id => {
                let missing = message.message_sequence.wrapping_sub(next);
                if missing > u32::MAX / 2 {
                    self.fail(LinkError::Protocol(format!(
                        "device message {} repeats or reorders boot {boot_id:016x}'s sequence",
                        message.message_sequence
                    )));
                    return;
                }
                self.health.missing = self.health.missing.saturating_add(missing);
            }
            _ => {
                self.health.boots = self.health.boots.saturating_add(1);
                self.hello = None;
            }
        }
        self.boot = Some((message.boot_id, message.message_sequence.wrapping_add(1)));
        if let Some(Ok(hello)) = message.decode::<Hello>() {
            self.hello = Some(Boot {
                boot_id: message.boot_id,
                hello,
            });
        }
        if self.received.len() < RECEIVED_CAPACITY {
            self.received.push_back(message);
        } else {
            self.health.dropped = self.health.dropped.saturating_add(1);
        }
    }

    /// The first kept message `matches` accepts, removed.
    fn take(&mut self, matches: impl Fn(&Received) -> bool) -> Option<Received> {
        let index = self.received.iter().position(matches)?;
        self.received.remove(index)
    }
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
    outbound: mpsc::Sender<Zeroizing<Vec<u8>>>,
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
        let (outbound, commands) = mpsc::channel();
        let poll = transport::events::EventPoll::new()
            .map_err(|error| LinkError::Transport(format!("cannot poll the serial line: {error}")))?;
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
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if let Some(boot) = state.hello {
                return Ok(boot);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(LinkError::NoHello);
            }
            if !asked && now >= ask_at {
                drop(state);
                // Discovery names no boot: any running boot answers it.
                self.send(0, GetHello)
                    .map_err(|_| LinkError::Protocol("GetHello does not encode".to_owned()))?;
                asked = true;
                state = self.shared.lock();
                continue;
            }
            state = self.wait(state, deadline.min(if asked { deadline } else { ask_at }))?;
        }
    }

    /// Send `request` to `boot` and wait up to `timeout` for its response.
    /// Other messages of the same request (related events) stay for
    /// [`Link::take`].
    pub fn call<E: Endpoint>(
        &self,
        boot: &Boot,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, CallError> {
        let request_id = self.send(boot.boot_id, request)?;
        let deadline = Instant::now() + timeout;
        let ours = |message: &Received| {
            message.boot_id == boot.boot_id
                && message.session_id == 0
                && message.request_id == request_id
                && (message.is::<E::Response>() || message.is::<Rejected>())
        };
        let mut state = self.shared.lock();
        loop {
            if let Some(message) = state.take(ours) {
                if let Some(rejected) = message.decode::<Rejected>() {
                    return Err(CallError::Rejected(rejected?.0));
                }
                return message
                    .decode::<E::Response>()
                    .unwrap_or(Err(CallError::Payload {
                        path: <E::Response as Message>::PATH,
                    }));
            }
            if let Some(error) = &state.failure {
                return Err(error.clone().into());
            }
            if let Some((boot_id, _)) = state.boot
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

    /// The first kept message of `boot` that `matches` accepts, waiting up
    /// to `timeout`; `None` when none came.
    pub fn take(
        &self,
        boot: &Boot,
        matches: impl Fn(&Received) -> bool,
        timeout: Duration,
    ) -> Result<Option<Received>, CallError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(message) =
                state.take(|message| message.boot_id == boot.boot_id && matches(message))
            {
                return Ok(Some(message));
            }
            if let Some(error) = &state.failure {
                return Err(error.clone().into());
            }
            if let Some((boot_id, _)) = state.boot
                && boot_id != boot.boot_id
            {
                return Err(CallError::Rebooted { boot_id });
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            state = self.wait(state, deadline)?;
        }
    }

    /// What the link observed of the wire so far.
    pub fn health(&self) -> Health {
        self.shared.lock().health
    }

    /// The device's diagnostic lines since the last call, as text.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().notes)
    }

    fn send<M: Message>(&self, boot_id: u64, message: M) -> Result<u32, CallError> {
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
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
                    .map_err(|_| CallError::Encode)?
                    .to_vec(),
            )
        };
        self.outbound
            .send(frame)
            .map_err(|_| LinkError::Transport("the link's worker ended".to_owned()))?;
        self.wake
            .wake()
            .map_err(|error| LinkError::Transport(format!("cannot wake the link: {error}")))?;
        Ok(request_id)
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
