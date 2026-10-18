//! Serial I/O and evidence persistence. Protocol operations live in `protocol`.

use super::protocol::decode_counters_are_clean;
use super::*;
use crate::transport::events::EventPoll;
use std::{
    collections::VecDeque,
    os::fd::{AsRawFd, FromRawFd as _, OwnedFd},
};

impl SerialCapture {
    /// Retain an admitted operation until this capture's serial worker closes.
    pub fn with_io_lifetime(mut self, lifetime: oer_process::IoLifetime) -> Self {
        self.io_lifetime = Some(lifetime);
        self
    }
    /// Report the decoded messages to `observer` when the capture persists.
    pub fn observed_by(mut self, observer: Box<dyn CaptureObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Observe an already-running target on the console `open` opens
    /// without touching its lines. Do not clear input, toggle reset lines,
    /// upload artifacts or initialize the runtime.
    pub fn attach(open: ConsoleOpener, output: &Path) -> Result<Self> {
        Self::start_transport_at(output, CaptureOrigin::Attachment, move || {
            let serial = open()
                .map_err(|error| LinkError::transport(format!("serial attach failed: {error}")))?;
            nonblocking_serial(serial)
        })
    }

    /// Own the output before opening and resetting the target, so even setup
    /// failures and early returns leave a capture and a structured diagnosis.
    pub fn start_with_reset(dut: &dyn Dut, output: &Path) -> Result<Self> {
        let port = dut.console().to_owned();
        let open = dut.console_with_reset();
        Self::start_transport(output, move || {
            let serial = open().map_err(|error| {
                LinkError::transport(format!(
                    "serial open and target reset failed for {}: {error}",
                    port.display()
                ))
            })?;
            nonblocking_serial(serial)
        })
    }

    fn start_transport<T: Read + Write + AsRawFd + 'static>(
        output: &Path,
        open: impl FnOnce() -> std::result::Result<T, LinkError> + Send + 'static,
    ) -> Result<Self> {
        Self::start_transport_at(output, CaptureOrigin::Boot, open)
    }

    fn start_transport_at<T: Read + Write + AsRawFd + 'static>(
        output: &Path,
        origin: CaptureOrigin,
        open: impl FnOnce() -> std::result::Result<T, LinkError> + Send + 'static,
    ) -> Result<Self> {
        fs::create_dir_all(output)?;
        let raw = fs::File::create(output.join("uart.bin"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let protocol = Arc::new(ProtocolEvents::default());
        protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .health
            .origin = origin;
        let (outbound, outbound_rx) = mpsc::channel();
        let poll = EventPoll::new()?;
        let worker_wake = poll.waker();
        let notify = Arc::clone(&protocol);
        let cancellation = oer_process::notify_on_cancel(move || {
            let _state = notify
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            notify.changed.notify_all();
        });
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let worker_protocol = Arc::clone(&protocol);
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut serial = open()?;
                capture_serial(
                    &mut serial,
                    raw,
                    &worker_stop,
                    &worker_bytes,
                    &worker_protocol,
                    outbound_rx,
                    poll,
                )
            }));
            let failure = match result {
                Ok(result) => result.err(),
                Err(_) => Some(LinkError::transport("serial worker panicked")),
            };
            worker_protocol.close(failure);
        });
        Ok(Self {
            io_lifetime: None,
            stop,
            bytes,
            protocol,
            outbound,
            worker_wake,
            _cancellation: cancellation,
            next_host_sequence: AtomicU32::new(1),
            next_session_id: AtomicU64::new(1),
            worker: Some(worker),
            output: output.to_owned(),
            persisted: false,
            observer: None,
            profile: None,
        })
    }

    /// Boot smoke precedes the typed radio runtime; its text marker is the
    /// contract for the relocated Embassy timer executing once.
    pub fn wait_for_boot_smoke(&self, timeout: Duration) -> Result<()> {
        const PASS: &[u8] = b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS";
        const PANIC: &[u8] = b"OPEN_RADIO_HIL runtime=PANIC";
        let deadline = crate::transport::events::deadline_after(timeout);
        let mut state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while Instant::now() < deadline {
            oer_process::check_cancelled()?;
            state.check()?;
            let bytes = self
                .bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bytes
                .windows(PANIC.len())
                .any(|candidate| candidate == PANIC)
            {
                return Err("boot-smoke runtime panicked".into());
            }
            if bytes.windows(PASS.len()).any(|candidate| candidate == PASS) {
                return Ok(());
            }
            drop(bytes);
            (state, _) = self
                .protocol
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        Err("boot-smoke timer completion was not observed".into())
    }

    pub fn finish_with<T>(self, result: Result<T>) -> Result<T> {
        Self::combine_result(result, self.finish())
    }

    pub fn finish_observation_with<T>(self, result: Result<T>) -> Result<T> {
        Self::combine_result(result, self.finish_capture(false))
    }

    fn combine_result<T>(result: Result<T>, finalization: Result<String>) -> Result<T> {
        match (result, finalization) {
            (Ok(value), Ok(_)) => Ok(value),
            (Err(primary), Ok(_)) => Err(primary),
            (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(finalization)) => Err(Box::new(error::FinalizationError {
                primary,
                finalization,
            })),
        }
    }

    /// Finish the exchange, then persist evidence before returning any link
    /// failure. `uart.bin` contains only received bytes; `uart.log` is a lossy
    /// text view. Host commands are never recorded (they may hold credentials).
    pub fn finish(self) -> Result<String> {
        self.finish_capture(true)
    }

    /// Observation includes counters from before this attachment. Persist them
    /// without interpreting historical target errors as a new scenario failure.
    fn finish_capture(mut self, qualify_target_health: bool) -> Result<String> {
        // A target that left USB as expected has no link left to ask.
        let active = {
            let state = self
                .protocol
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.health.active && !state.closed && !state.expected_detach
        };
        let target_health = if active && self.check_link().is_ok() {
            Some(self.query_link_health(PROTOCOL_READY_TIMEOUT))
        } else {
            None
        };
        if let Some(profile) = self.profile.take() {
            // A profile is diagnostic: a drain that fails is recorded, not a
            // failure of the workload it observed.
            let drained = if active && self.check_link().is_ok() {
                self.drain_profile()
            } else {
                Err("the link ended before the profile was drained".into())
            };
            let record = match drained {
                Ok(profile_record) => serde_json::json!({"schema": 1, "request": profile.request,
                    "status": profile_record.0, "samples": profile_record.1}),
                Err(error) => serde_json::json!({"schema": 1, "request": profile.request,
                    "error": error.to_string()}),
            };
            oer_durable::atomic_json(&self.output.join("profile.json"), &record)?;
        }
        self.stop_and_join();
        let uart = self.persist(target_health.as_ref(), true)?;
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = &state.failure {
            return Err(error.clone().into());
        }
        if !decode_counters_are_clean(state.health.counters) {
            return Err(LinkError::protocol(format!(
                "host reported unhealthy HIL frame decoding: {:?}",
                state.health.counters,
            ))
            .into());
        }
        if let Some(result) = target_health {
            let health = result?;
            if qualify_target_health {
                protocol::validate_target_link_health(health)?;
            }
            if health.text_dropped != 0 || health.text_truncated != 0 {
                eprintln!(
                    "diagnostic_text_loss=dropped:{} truncated:{}",
                    health.text_dropped, health.text_truncated
                );
            }
        }
        Ok(uart)
    }

    pub(super) fn persist(
        &mut self,
        target_health: Option<&Result<LinkHealth>>,
        finalized: bool,
    ) -> Result<String> {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let uart = String::from_utf8_lossy(&bytes).into_owned();
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Return decoded observations to the repetition before fallible disk
        // writes. Even a capture storage failure must retain available values.
        let observations = self
            .observer
            .as_ref()
            .map(|observer| observer.observe(&state.messages, bytes.len() as u64));
        // Rewrite from memory as well: a failed raw-file write still gets a
        // final attempt to preserve every byte accepted by the serial reader.
        oer_durable::atomic_write(&self.output.join("uart.bin"), &bytes)?;
        oer_durable::atomic_write(&self.output.join("uart.log"), uart.as_bytes())?;
        let mut log = Vec::new();
        for sent in &state.sent {
            serde_json::to_writer(
                &mut log,
                &serde_json::json!({"record": "host-command", "command": sent}),
            )?;
            log.push(b'\n');
        }
        for (index, message) in state.messages.iter().enumerate() {
            serde_json::to_writer(
                &mut log,
                &serde_json::json!({
                    "record": "target-event",
                    "message": message.to_json(),
                    "host_received_unix_micros": state.received_unix_micros.get(index),
                }),
            )?;
            log.push(b'\n');
        }
        for record in [
            serde_json::json!({"record": "link-health", "health": state.health}),
            serde_json::json!({"record": "capture-end", "finalized": finalized, "cancelled": oer_process::cancellation_requested(), "failure": state.failure}),
            serde_json::json!({"record": "target-link-health", "health": target_health.map(|result|
                result.as_ref().map_err(|error| error.to_string()))}),
        ] {
            serde_json::to_writer(&mut log, &record)?;
            log.push(b'\n');
        }
        oer_durable::atomic_write(&self.output.join("protocol.jsonl"), &log)?;
        if let Some(observations) = observations {
            oer_durable::atomic_json(
                &self.output.join("measurements.json"),
                &serde_json::json!({
                    "schema": 1, "finalized": finalized, "failure": state.failure,
                    "measurements": observations,
                }),
            )?;
        }
        self.persisted = true;
        Ok(uart)
    }

    pub(super) fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.worker_wake.wake();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            self.protocol
                .close(Some(LinkError::transport("serial worker panicked")));
        }
    }
}

fn nonblocking_serial(serial: Box<dyn SerialLine>) -> std::result::Result<fs::File, LinkError> {
    #[allow(
        unsafe_code,
        reason = "a serial port hands its descriptor over only as a raw fd"
    )]
    // SAFETY: `into_raw_descriptor` releases sole ownership of the open descriptor,
    // which this `OwnedFd` takes exactly once.
    let file = fs::File::from(unsafe { OwnedFd::from_raw_fd(serial.into_raw_descriptor()) });
    let flags = rustix::fs::fcntl_getfl(&file);
    flags
        .and_then(|flags| rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK))
        .map_err(|error| {
            LinkError::transport(format!("cannot configure nonblocking serial I/O: {error}"))
        })?;
    Ok(file)
}

fn capture_serial(
    serial: &mut (impl Read + Write + AsRawFd),
    mut raw: fs::File,
    stop: &AtomicBool,
    bytes: &Mutex<Vec<u8>>,
    protocol: &ProtocolEvents,
    outbound: mpsc::Receiver<Zeroizing<Vec<u8>>>,
    mut poll: EventPoll,
) -> std::result::Result<(), LinkError> {
    poll.register(serial, false)
        .map_err(|error| LinkError::transport(error.to_string()))?;
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0_u8; 2_048];
    let mut pending = VecDeque::<Zeroizing<Vec<u8>>>::new();
    let mut offset = 0;
    let mut write_deadline = None;
    let mut writable_interest = false;
    let mut readable = false;
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        pending.extend(outbound.try_iter());
        while let Some(frame) = pending.front() {
            let deadline =
                *write_deadline.get_or_insert_with(|| Instant::now() + PROTOCOL_READY_TIMEOUT);
            if Instant::now() >= deadline {
                return Err(LinkError::transport(
                    "serial command write deadline exceeded",
                ));
            }
            match serial.write(&frame[offset..]) {
                Ok(0) => return Err(LinkError::transport("serial writer made no progress")),
                Ok(length) => {
                    offset += length;
                    if offset == frame.len() {
                        pending.pop_front();
                        offset = 0;
                        write_deadline = None;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(LinkError::transport(format!(
                        "serial write failed: {error}"
                    )));
                }
            }
        }
        let wants_write = !pending.is_empty();
        if wants_write != writable_interest {
            poll.interest(serial, wants_write)
                .map_err(|error| LinkError::transport(error.to_string()))?;
            writable_interest = wants_write;
        }
        if !readable {
            readable = poll
                .wait(write_deadline)
                .map_err(|error| LinkError::transport(error.to_string()))?
                .readable;
            if !readable {
                continue;
            }
        }
        let length = match serial.read(&mut buffer) {
            Ok(0) => return Err(LinkError::transport("serial reader reached end of stream")),
            Ok(length) => length,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                readable = false;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(LinkError::transport(format!("serial read failed: {error}"))),
        };
        let chunk = &buffer[..length];
        append(bytes, chunk);
        raw.write_all(chunk).map_err(|error| {
            LinkError::transport(format!("cannot persist raw serial bytes: {error}"))
        })?;
        let mut state = protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before_read = decoder.counters();
        decoder.feed(oer_hil_protocol::WireKind::Event, chunk, |frame| {
            match frame.map(|frame| super::Received::from_frame(&frame)) {
                Ok(Ok(message)) => {
                    if let Some(boot_id) = state.health.boot_id
                        && boot_id != message.boot_id
                        && !state.accept_expected_reboot(&message, Instant::now())
                    {
                        state.fail(LinkError::protocol(format!(
                            "target rebooted during capture: boot {boot_id} became {}",
                            message.boot_id,
                        )));
                    }
                    state.health.observe(&message, before_read);
                    if state.messages.len() < PROTOCOL_EVENT_CAPACITY {
                        state.messages.push(message);
                        state.received_unix_micros.push(super::host_unix_micros());
                    } else {
                        state.health.fail(format!(
                            "host protocol event capacity {PROTOCOL_EVENT_CAPACITY} exhausted"
                        ));
                    }
                }
                // Nothing can decode or check a message whose key this host
                // does not know: the capture cannot vouch for it.
                Ok(Err(unknown)) => state.health.fail(unknown),
                // Another framing is an image of another protocol: actionable
                // even before Hello. Arbitrary boot text is not a wire failure.
                Err(error @ oer_hil_protocol::DecodeError::FramingVersion) => {
                    state.health.fail(error.to_string());
                }
                Err(error) => state.health.decode_error(error),
            }
            if let Some(error) = state.health.failure.clone() {
                state.fail(LinkError::protocol(error));
            }
        });
        state.health.update_decoder_counters(decoder.counters());
        if state.health.active && !decode_counters_are_clean(state.health.counters) {
            let counters = state.health.counters;
            state.fail(LinkError::protocol(format!(
                "host HIL frame decoding failed: {counters:?}"
            )));
        }
        protocol.changed.notify_all();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
