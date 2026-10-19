use super::test_support::*;
use super::*;
use crate::error::ErrorKind;
use std::io;

#[test]
fn the_observer_sees_every_message_even_when_the_link_fails() {
    struct Count(Arc<Mutex<Option<(usize, u64)>>>);
    impl CaptureObserver for Count {
        fn observe(&self, messages: &[Received], received_bytes: u64) -> serde_json::Value {
            *self.0.lock().unwrap() = Some((messages.len(), received_bytes));
            serde_json::json!([{"name": "messages", "value": messages.len()}])
        }
    }
    let output = Output::new();
    let seen = Arc::new(Mutex::new(None));
    let (capture, input) = capture(&output, false);
    let capture = capture.observed_by(Box::new(Count(Arc::clone(&seen))));
    activate(&capture, &input);
    input.send(Err(io::ErrorKind::BrokenPipe.into())).unwrap();
    failure(&capture, ErrorKind::Transport);
    drop(capture);
    let (messages, bytes) = seen.lock().unwrap().unwrap();
    assert_eq!(messages, 1, "the boot's hello");
    assert!(bytes > 0);
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(output.0.join("measurements.json")).unwrap()).unwrap();
    assert_eq!(stored["finalized"], false);
    assert!(!stored["failure"].is_null());
    assert_eq!(stored["measurements"][0]["value"], 1);
}

#[test]
fn observation_discovers_a_running_boot_without_initializing_or_clearing_results() {
    let output = Output::new();
    let (input, rx) = serial_pair();
    let (writes, commands) = mpsc::channel();
    let capture =
        SerialCapture::start_transport_at(&output.0, CaptureOrigin::Attachment, move || {
            Ok(Serial {
                input: rx,
                fail_write: false,
                writes: Some(writes),
            })
        })
        .unwrap();
    let input_guard = input.clone();
    let target = thread::spawn(move || {
        fn reply<M: oer_hil_protocol::Message>(
            input: &Input,
            request: &Received,
            sequence: u32,
            body: M,
        ) {
            input
                .send(Ok(frame(Envelope::new(
                    7,
                    sequence,
                    0,
                    request.request_id,
                    body,
                ))))
                .unwrap();
        }
        let request = receive_request(&commands);
        assert!(request.is::<oer_hil_protocol::base::GetHello>());
        assert_eq!((request.boot_id, request.session_id), (0, 0));
        reply(&input, &request, 87, hello(7, 87).body);
        let mut sequence = answer_image_keys(&input, &commands, 7, 88);
        let (request, oer_hil_protocol::network::GetStatus) = receive(&commands);
        assert_eq!((request.boot_id, request.session_id), (7, 0));
        let status = OperationStatus {
            state: oer_hil_protocol::network::SessionState::Finished,
            configured_session_id: Some(9),
            completed_session_id: Some(9),
        };
        reply(
            &input,
            &request,
            sequence,
            oer_hil_protocol::network::Status(status),
        );
        sequence += 1;
        for _ in 0..2 {
            let request = receive_request(&commands);
            assert!(request.is::<oer_hil_protocol::base::GetLinkHealth>());
            assert_eq!(request.boot_id, 7);
            let health = oer_hil_protocol::base::LinkHealth {
                rx_frames: 12,
                rx_cobs_errors: 0,
                rx_checksum_errors: 2,
                rx_decode_errors: 0,
                rx_overflows: 0,
                tx_frames: sequence,
                tx_dropped: 0,
                text_dropped: 0,
                text_truncated: 0,
            };
            reply(&input, &request, sequence, health);
            sequence += 1;
        }
    });
    let report = capture.observe(Duration::from_secs(2));
    let report = capture.finish_observation_with(report).unwrap();
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(report["boot_id"], 7);
    assert_eq!(report["operation"]["completed_session_id"], 9);
    assert!(report["stack"].is_null());
    assert_eq!(report["link"]["rx_checksum_errors"], 2);
    target.join().unwrap();
    drop(input_guard);
    assert!(
        fs::read_to_string(output.0.join("protocol.jsonl"))
            .unwrap()
            .contains("base/link-health")
    );
}

fn failure(capture: &SerialCapture, kind: ErrorKind) -> String {
    let start = Instant::now();
    let error = capture
        .wait_for_message_after(0, Duration::from_secs(3), |_| false)
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "failure waited for the event deadline"
    );
    assert_eq!(error.downcast_ref::<LinkError>().unwrap().kind, kind);
    error.to_string()
}

#[test]
fn open_failure_wakes_hello_wait_and_survives_finish() {
    let output = Output::new();
    let capture = SerialCapture::start_transport::<Serial>(&output.0, || {
        Err(LinkError::transport("cannot open test device"))
    })
    .unwrap();
    assert_eq!(
        failure(&capture, ErrorKind::Transport),
        "cannot open test device"
    );
    assert_eq!(
        capture.finish().unwrap_err().to_string(),
        "cannot open test device"
    );
    assert!(fs::read(output.0.join("uart.bin")).unwrap().is_empty());
    assert!(
        fs::read_to_string(output.0.join("protocol.jsonl"))
            .unwrap()
            .contains("cannot open test device")
    );
}

#[test]
fn read_failure_preserves_exact_partial_bytes_on_early_return() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    let raw = b"boot\xff\n\x00\x00".to_vec();
    input.send(Ok(raw.clone())).unwrap();
    input
        .send(Err(io::ErrorKind::ConnectionReset.into()))
        .unwrap();
    assert!(failure(&capture, ErrorKind::Transport).contains("serial read failed"));
    // The raw evidence already exists before finish or Drop.
    assert_eq!(fs::read(output.0.join("uart.bin")).unwrap(), raw);
    drop(capture);
    assert_eq!(fs::read(output.0.join("uart.bin")).unwrap(), raw);
    let log = fs::read_to_string(output.0.join("uart.log")).unwrap();
    assert_eq!(log, String::from_utf8_lossy(&raw));
    assert!(!log.contains("serial read failed"));
}

#[test]
fn end_of_stream_is_a_transport_failure() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    drop(input);
    assert!(failure(&capture, ErrorKind::Transport).contains("end of stream"));
}

#[test]
fn worker_panic_wakes_waiters() {
    let output = Output::new();
    let capture =
        SerialCapture::start_transport::<Serial>(&output.0, || panic!("injected worker panic"))
            .unwrap();
    assert_eq!(
        failure(&capture, ErrorKind::Transport),
        "serial worker panicked"
    );
}

#[test]
fn write_failure_reaches_the_command_caller() {
    let output = Output::new();
    let (capture, input) = capture(&output, true);
    activate(&capture, &input);
    let start = Instant::now();
    let error = capture
        .request_image_keys(Duration::from_secs(3))
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(
        error.downcast_ref::<LinkError>().unwrap().kind,
        ErrorKind::Transport
    );
    assert!(error.to_string().contains("serial write failed"));
}

#[test]
fn malformed_active_frame_wakes_waiters_and_preserves_the_frame() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input.send(Ok(vec![0, 0, 255, 1, 0])).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("decode failure"));
    drop(capture);
    assert!(
        fs::read(output.0.join("uart.bin"))
            .unwrap()
            .ends_with(&[0, 0, 255, 1, 0])
    );
}

#[test]
fn reboot_cannot_clear_a_failure_or_satisfy_an_old_operation() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let mut batch = frame(Envelope::new(7, 3, 0, 0, oer_hil_protocol::base::Accepted));
    batch.extend(frame(hello(8, 0)));
    input.send(Ok(batch)).unwrap();
    let cause = failure(&capture, ErrorKind::Protocol);
    assert!(cause.contains("sequence discontinuity"));
    assert_eq!(capture.finish().unwrap_err().to_string(), cause);
}

#[test]
fn unexpected_reboot_fails_the_capture() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input.send(Ok(frame(hello(8, 0)))).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("target rebooted"));
}

#[test]
fn optional_wait_reports_absence_only_while_the_link_is_healthy() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    assert!(
        capture
            .wait_for_message_after(0, Duration::ZERO, |_| true)
            .unwrap()
            .is_none()
    );
    activate(&capture, &input);
    assert!(
        capture
            .wait_for_message_after(
                0,
                Duration::ZERO,
                Received::is::<oer_hil_protocol::base::Hello>
            )
            .unwrap()
            .is_some()
    );
    assert!(
        capture
            .wait_for_message_after(1, Duration::from_millis(5), |_| true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_message_this_host_does_not_know_fails_the_link() {
    /// A message of some other revision's wire.
    #[derive(serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
    struct Stranger;
    impl oer_hil_protocol::Message for Stranger {
        const PATH: &'static str = "base/stranger";
    }
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input
        .send(Ok(frame(Envelope::new(7, 1, 0, 0, Stranger))))
        .unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("does not know"));
}

#[test]
fn receive_overflow_without_a_completed_frame_wakes_waiters() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let mut bytes = vec![0, 0];
    bytes.extend(vec![1; 1800]);
    input.send(Ok(bytes)).unwrap();
    assert!(failure(&capture, ErrorKind::Protocol).contains("overflows: 1"));
}

#[test]
fn finalization_failure_keeps_the_primary_cause_and_both_messages() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    drop(input);
    failure(&capture, ErrorKind::Transport);
    let error = capture
        .finish_with::<()>(Err("scenario criterion failed".into()))
        .unwrap_err();
    assert!(error.to_string().starts_with("scenario criterion failed;"));
    assert!(error.to_string().contains("end of stream"));
    assert!(!is_link_failure(&*error));
    let records = fs::read_to_string(output.0.join("protocol.jsonl")).unwrap();
    assert!(records.contains("end of stream"));
}

struct CountedStream {
    stream: std::os::unix::net::UnixStream,
    reads: Arc<AtomicU32>,
}
impl AsRawFd for CountedStream {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.stream.as_raw_fd()
    }
}
impl Read for CountedStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.stream.read(bytes)
    }
}
impl Write for CountedStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[test]
fn queued_commands_make_progress_without_inbound_bytes_or_read_timeouts() {
    let output = Output::new();
    let (mut host, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
    host.set_nonblocking(true).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    // Fill the real kernel send buffer to exercise partial writes and POLLOUT.
    let mut padding = 0;
    loop {
        match host.write(&[0x11; 4096]) {
            Ok(length) => padding += length,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            result => panic!("unexpected prefill result: {result:?}"),
        }
    }
    let reads = Arc::new(AtomicU32::new(0));
    let worker_reads = Arc::clone(&reads);
    let capture = SerialCapture::start_transport(&output.0, move || {
        Ok(CountedStream {
            stream: host,
            reads: worker_reads,
        })
    })
    .unwrap();
    let frames = [vec![0x22; 80_000], vec![0x33; 80_000]];
    for frame in &frames {
        capture
            .outbound
            .send(Zeroizing::new(frame.clone()))
            .unwrap();
    }
    capture.worker_wake.wake().unwrap();
    let mut received = vec![0; padding + 160_000];
    peer.read_exact(&mut received).unwrap();
    assert!(received[..padding].iter().all(|&byte| byte == 0x11));
    assert_eq!(&received[padding..padding + 80_000], &frames[0]);
    assert_eq!(&received[padding + 80_000..], &frames[1]);
    assert_eq!(
        reads.load(Ordering::Relaxed),
        0,
        "idle serial input must not be polled"
    );
    let started = Instant::now();
    drop(capture);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown must wake the reactor"
    );
    assert!(
        fs::read(output.0.join("uart.bin")).unwrap().is_empty(),
        "outbound data must not enter the transcript"
    );
}

/// A target that answers the host's hello request of boot `boot` with its
/// Hello at target message `sequence`, then its capability pages. The thread
/// hands its end of the link back, so the link stays open until the test
/// joins it.
fn answer_hello(
    input: Input,
    writes: std::sync::mpsc::Receiver<Vec<u8>>,
    boot: u64,
    sequence: u32,
) -> std::thread::JoinHandle<Input> {
    std::thread::spawn(move || {
        answer_hello_request(&input, &writes, boot, sequence);
        answer_image_keys(&input, &writes, boot, sequence + 1);
        input
    })
}

/// Answer the host's next request, a hello request, as boot `boot`.
fn answer_hello_request(
    input: &Input,
    writes: &std::sync::mpsc::Receiver<Vec<u8>>,
    boot: u64,
    sequence: u32,
) {
    let request = receive_request(writes);
    assert!(request.is::<oer_hil_protocol::base::GetHello>());
    let mut answer = hello(boot, sequence);
    answer.request_id = request.request_id;
    input.send(Ok(frame(answer))).unwrap();
}

/// Wait until the capture has taken in `bytes`, however loaded the host.
fn wait_for_console(capture: &SerialCapture, bytes: &[u8]) {
    let started = std::time::Instant::now();
    while !capture
        .bytes
        .lock()
        .unwrap()
        .windows(bytes.len())
        .any(|window| window == bytes)
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "console bytes never arrived"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A Hello whose first bytes the link dropped, as the USB Serial/JTAG does.
fn truncated_hello(boot: u64) -> Vec<u8> {
    frame(hello(boot, 0))[4..].to_vec()
}

#[test]
fn a_boot_whose_hello_the_link_lost_begins_with_its_capability_answer() {
    let output = Output::new();
    let (capture, input, writes) = capture_with_commands(&output);
    let boot_line = b"I (236) boot: Loaded app from partition at offset 0x10000\r\n";
    input.send(Ok(boot_line.to_vec())).unwrap();
    // The capture must have read the boot's line before it looks for it.
    wait_for_console(&capture, boot_line);
    input.send(Ok(truncated_hello(9))).unwrap();
    let target = answer_hello(input, writes, 9, 1);
    let capabilities = capture.request_image_keys(Duration::from_secs(1)).unwrap();
    target.join().unwrap();
    assert!(capabilities.has::<oer_hil_protocol::system::WatchdogTest>());
    let health = capture.protocol.state.lock().unwrap().health.clone();
    assert_eq!(health.boot_id, Some(9));
    assert_eq!(
        health.solicited_hello,
        Some(crate::SolicitedHello {
            message_sequence: 1
        })
    );
    assert!(health.failure.is_none());
}

#[test]
fn a_lost_hello_stands_without_a_boot_on_the_console() {
    let output = Output::new();
    let (capture, input, writes) = capture_with_commands(&output);
    input.send(Ok(truncated_hello(9))).unwrap();
    let error = capture
        .request_image_keys(Duration::from_millis(200))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("did not publish a HIL protocol hello")
    );
    // The host asked nothing: an old boot could have answered.
    assert!(writes.try_recv().is_err());
}

#[test]
fn an_answer_late_in_a_boot_begins_no_boot() {
    let output = Output::new();
    let (capture, input, writes) = capture_with_commands(&output);
    input.send(Ok(b"ESP-ROM:chip-a\r\n".to_vec())).unwrap();
    wait_for_console(&capture, b"ESP-ROM:");
    // The answer begins no boot, so the host asks for no capability page.
    let target = std::thread::spawn(move || {
        answer_hello_request(&input, &writes, 9, 40);
        input
    });
    assert!(capture.request_image_keys(Duration::from_secs(1)).is_err());
    target.join().unwrap();
}

/// Whether a link failure caused `error`: what the runner classifies as an
/// infrastructure failure of the link.
fn is_link_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if error.is::<crate::error::LinkError>() {
            return true;
        }
        cause = error.source();
    }
    false
}
