//! The host/target link of a HIL run: one session with the board under test
//! (its UART capture, the generic protocol exchange [`SerialCapture::call`] /
//! [`SerialCapture::request`], readiness and reboots) and the line console
//! of a reference peer board ([`peer`]).
//!
//! The link reaches the board only through the [`Dut`] port the laboratory
//! implements; it depends on no laboratory, board,
//! scenario, run-bundle or image code. Families build their operations on
//! the generic exchange, a repetition's measurements observe a capture
//! through [`CaptureObserver`], and network traffic sessions live in
//! `oer-hil-net-traffic`.

use std::{
    fs,
    io::{Read, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    DecodeCounters, Envelope, FrameDecoder, FrameEncoder, base::LinkHealth,
    network::OperationStatus, network::WifiNetworkInterface,
};
use zeroize::Zeroizing;

mod target;
pub use target::{ConsoleOpener, Dut, SerialLine};
mod reboot;
use reboot::{ExpectedReboot, RebootObservation};
mod received;
pub use received::{Received, message_info};

/// How long the link waits for a protocol readiness step: a Hello or an
/// accepted command.
pub const PROTOCOL_READY_TIMEOUT: Duration = Duration::from_secs(10);
const PROTOCOL_EVENT_CAPACITY: usize = 16_384;

#[derive(Default)]
struct ProtocolEvents {
    state: Mutex<ProtocolState>,
    changed: Condvar,
}

#[derive(Default)]
struct ProtocolState {
    expected_reboot: Option<ExpectedReboot>,
    observed_reboot: Option<RebootObservation>,
    messages: Vec<Received>,
    /// When the host decoded each of `messages`, in Unix microseconds.
    received_unix_micros: Vec<u64>,
    /// Every command the host sent, in order.
    sent: Vec<SentCommand>,
    health: ProtocolHealth,
    failure: Option<LinkError>,
    closed: bool,
    /// The target was asked to leave USB: the end of its stream is that
    /// departure, not a transport failure.
    expected_detach: bool,
}

/// A command the host sent: its identity and when, never its payload, which
/// can carry credentials.
#[derive(serde::Serialize)]
struct SentCommand {
    request_id: u32,
    session_id: u64,
    /// The command's message path.
    path: &'static str,
    host_sent_unix_micros: u64,
}

/// Microseconds since the Unix epoch on the host clock.
fn host_unix_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as u64)
}

/// What a sent message was: its path.
impl ProtocolState {
    fn check(&self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        // After an expected detach, the messages received before it still
        // answer their requests.
        if self.closed && !self.expected_detach {
            return Err(LinkError::transport("serial capture is closed").into());
        }
        Ok(())
    }

    fn fail(&mut self, error: LinkError) {
        // A later reset or teardown must not replace the original cause.
        self.failure.get_or_insert(error);
    }
}

impl ProtocolEvents {
    fn close(&self, error: Option<LinkError>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = error
            && !state.expected_detach
        {
            state.fail(error);
        }
        state.closed = true;
        self.changed.notify_all();
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
struct ProtocolHealth {
    origin: CaptureOrigin,
    active: bool,
    boot_id: Option<u64>,
    next_sequence: u32,
    counters: DecodeCounters,
    #[serde(skip)]
    decoder_baseline: DecodeCounters,
    failure: Option<String>,
    /// The boot began with the answer to the host's capability request,
    /// because the link lost the boot's own Hello.
    #[serde(skip_serializing_if = "Option::is_none")]
    solicited_hello: Option<SolicitedHello>,
    /// Whether a boot may begin with a solicited Hello; set only while the
    /// host asks a boot whose Hello it missed.
    #[serde(skip)]
    accept_solicited_hello: bool,
}

/// A boot that began with the answer to the host's capability request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
struct SolicitedHello {
    /// The answer's target message sequence: the boot's messages before it
    /// were lost on the link.
    message_sequence: u32,
}

/// The latest target message sequence a solicited Hello may begin a boot
/// at: the boot's own Hello and the few messages a boot sends unasked.
const SOLICITED_HELLO_SEQUENCE_LIMIT: u32 = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CaptureOrigin {
    #[default]
    Boot,
    Attachment,
}

impl ProtocolHealth {
    fn observe(&mut self, message: &Received, decoder_counters: DecodeCounters) {
        match self.boot_id {
            None => {
                self.begin_boot(message, decoder_counters);
            }
            Some(boot_id) if boot_id != message.boot_id => {
                self.begin_boot(message, decoder_counters);
            }
            Some(_) if message.message_sequence != self.next_sequence => {
                let expected = self.next_sequence;
                self.next_sequence = message.message_sequence.wrapping_add(1);
                self.fail(format!(
                    "target message sequence discontinuity: expected {expected}, observed {}",
                    message.message_sequence
                ));
            }
            Some(_) => self.next_sequence = self.next_sequence.wrapping_add(1),
        }
    }

    fn begin_boot(&mut self, message: &Received, decoder_counters: DecodeCounters) {
        self.active = true;
        self.boot_id = Some(message.boot_id);
        self.next_sequence = message.message_sequence.wrapping_add(1);
        self.decoder_baseline = decoder_counters;
        self.counters = DecodeCounters::default();
        self.failure = None;
        if message.boot_id == 0 {
            self.fail("target published a reserved zero boot identity".into());
        } else if self.origin == CaptureOrigin::Boot
            && (message.message_sequence != 0 || !message.is::<oer_hil_protocol::base::Hello>())
        {
            if self.accept_solicited_hello
                && message.is::<oer_hil_protocol::base::Hello>()
                && message.request_id != 0
                && message.message_sequence <= SOLICITED_HELLO_SEQUENCE_LIMIT
            {
                self.solicited_hello = Some(SolicitedHello {
                    message_sequence: message.message_sequence,
                });
                return;
            }
            self.fail(format!(
                "boot {} began with {} at target message sequence {}, expected Hello at 0",
                message.boot_id,
                message.path(),
                message.message_sequence
            ));
        }
    }

    fn update_decoder_counters(&mut self, totals: DecodeCounters) {
        self.counters = decode_counter_delta(totals, self.decoder_baseline);
    }

    fn decode_error(&mut self, error: impl std::fmt::Display) {
        if self.active {
            self.fail(format!(
                "HIL wire decode failure after protocol activation: {error}"
            ));
        }
    }

    fn fail(&mut self, failure: String) {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
    }
}

fn decode_counter_delta(totals: DecodeCounters, baseline: DecodeCounters) -> DecodeCounters {
    DecodeCounters {
        frames: totals.frames.saturating_sub(baseline.frames),
        cobs_errors: totals.cobs_errors.saturating_sub(baseline.cobs_errors),
        too_short: totals.too_short.saturating_sub(baseline.too_short),
        header_errors: totals.header_errors.saturating_sub(baseline.header_errors),
        framing_version_errors: totals
            .framing_version_errors
            .saturating_sub(baseline.framing_version_errors),
        message_kind_errors: totals
            .message_kind_errors
            .saturating_sub(baseline.message_kind_errors),
        payload_length_errors: totals
            .payload_length_errors
            .saturating_sub(baseline.payload_length_errors),
        checksum_errors: totals
            .checksum_errors
            .saturating_sub(baseline.checksum_errors),
        deserialize_errors: totals
            .deserialize_errors
            .saturating_sub(baseline.deserialize_errors),
        overflows: totals.overflows.saturating_sub(baseline.overflows),
    }
}

/// Concurrent UART transcript retained across traffic setup and measurement.
pub struct SerialCapture {
    // Released after Drop joins the serial worker and closes its descriptor.
    io_lifetime: Option<oer_process::IoLifetime>,
    stop: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
    protocol: Arc<ProtocolEvents>,
    outbound: mpsc::Sender<Zeroizing<Vec<u8>>>,
    worker_wake: Arc<mio::Waker>,
    _cancellation: oer_process::CancellationNotification,
    next_host_sequence: AtomicU32,
    next_session_id: AtomicU64,
    worker: Option<thread::JoinHandle<()>>,
    output: PathBuf,
    persisted: bool,
    observer: Option<Box<dyn CaptureObserver>>,
    /// The armed program-counter profile and the request it records, drained
    /// when the capture finishes.
    profile: Option<Profile>,
}

fn command_response_matches(
    message: &Received,
    boot_id: u64,
    session_id: u64,
    request_id: u32,
) -> bool {
    message.boot_id == boot_id
        && message.session_id == session_id
        && message.request_id == request_id
}

/// A command the device accepted: the messages that complete it carry its
/// boot, request and session zero, after the acceptance.
#[derive(Clone, Copy, Debug)]
pub struct CommandHandle {
    path: &'static str,
    boot_id: u64,
    request_id: u32,
    first_event: usize,
}

impl CommandHandle {
    /// The cursor just after the command's acceptance.
    pub fn first_event(&self) -> usize {
        self.first_event
    }

    pub fn request_id(&self) -> u32 {
        self.request_id
    }

    /// Whether `message` belongs to this command.
    pub fn correlates(&self, message: &Received) -> bool {
        message.boot_id == self.boot_id
            && message.session_id == 0
            && message.request_id == self.request_id
    }

    /// A handle of a command of `path` the tests accepted by hand.
    #[cfg(any(test, feature = "test-support"))]
    pub fn accepted(path: &'static str, boot_id: u64, request_id: u32, first_event: usize) -> Self {
        Self {
            path,
            boot_id,
            request_id,
            first_event,
        }
    }
}

/// What a capture reports, when it persists, about the messages it decoded:
/// the repetition's measurement projection implements it.
pub trait CaptureObserver: Send + Sync {
    /// Project `messages` (and the count of bytes received) into the
    /// document `measurements.json` keeps; it is called once, before any
    /// fallible write, so the projection survives a storage failure.
    fn observe(&self, messages: &[Received], received_bytes: u64) -> serde_json::Value;
}

/// A program-counter profile a capture arms after the boot's Hello: the
/// target's arming command and the scenario's request `profile.json` records.
#[derive(Clone, Debug)]
pub struct Profile {
    pub control: oer_hil_protocol::telemetry::ProfileControl,
    pub request: serde_json::Value,
}

impl Drop for SerialCapture {
    fn drop(&mut self) {
        self.stop_and_join();
        if !self.persisted
            && let Err(error) = self.persist(None, false)
        {
            eprintln!("cannot preserve partial serial capture: {error}");
        }
    }
}

fn append(bytes: &Mutex<Vec<u8>>, chunk: &[u8]) {
    bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend_from_slice(chunk);
}

mod capture;
#[cfg(any(test, feature = "test-support"))]
pub use capture::test_support;
pub mod error;
use error::LinkError;
mod protocol;
pub use protocol::latest_boot_id_in;
#[cfg(test)]
mod tests;

pub mod peer;
pub mod transport;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
