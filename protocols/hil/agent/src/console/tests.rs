use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use crate::test_messages::{Probe, ProbeArmed, ProbeMode};
use embassy_futures::block_on;
use embassy_futures::select::{Either3, select3};
use oer_protocols_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetImageKeys, GetLinkHealth, Hello, ImageKeySet,
    PostMortemCheckpoints, RejectReason, ResetReason, sorted_keys,
};
use oer_protocols_hil_protocol::{Envelope, FrameDecoder, Key, WireKind};

use super::*;
use crate::mailbox::{Inbox, Mailbox, Trap};

crate::requests! {
    enum WatchdogRequest (sessions = false) {
        Test(Probe),
    }
}

crate::requests! {
    /// A set that may trap after an answer, as the fault service's.
    #[expect(dead_code, reason = "the trap tests answer without reading the probe")]
    enum TrappingRequest (sessions = false, traps) {
        Test(Probe),
    }
}

const BOOT: u64 = 77;

/// The early writer of a test thread: the bytes it dropped.
struct Captured;

impl Immediate for Captured {
    fn take_lost() -> u32 {
        EARLY_LOST.replace(0)
    }
}

std::thread_local! {
    static EARLY_LOST: Cell<u32> = const { Cell::new(0) };
}

struct Chip;

impl Platform for Chip {
    fn boot_evidence_written(&self) {}
    fn boot_evidence(&self) -> BootEvidence {
        BootEvidence {
            reset_reason: ResetReason::Software,
            raw_reset_reason: 3,
            post_mortem: None,
            platform_panic: None,
        }
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}

const SERVED: [Key; 6] = sorted_keys([
    GetImageKeys::KEY,
    GetBootStatus::KEY,
    GetLinkHealth::KEY,
    Hello::KEY,
    Probe::KEY,
    ProbeArmed::KEY,
]);

/// The host side of the endpoint: bytes to deliver and bytes written.
#[derive(Clone, Default)]
struct Host {
    to_device: Rc<RefCell<VecDeque<Vec<u8>>>>,
    from_device: Rc<RefCell<Vec<u8>>>,
}

impl Host {
    fn send<M: Message>(&self, session: u64, request: u32, body: M) {
        let mut encoder = FrameEncoder::new();
        let bytes = encoder
            .encode(&Envelope::new(BOOT, 0, session, request, body))
            .unwrap()
            .to_vec();
        self.to_device.borrow_mut().push_back(bytes);
    }

    /// Every frame the device wrote: its request id, sequence and key.
    fn frames(&self) -> Vec<(u32, u32, Key)> {
        let mut decoder = FrameDecoder::new();
        let mut frames = Vec::new();
        decoder.feed(WireKind::Event, &self.from_device.borrow(), |frame| {
            let frame = frame.unwrap();
            assert_eq!(frame.boot_id, BOOT);
            frames.push((frame.request_id, frame.message_sequence, frame.key));
        });
        frames
    }
}

/// The device's one duplex endpoint over the host's queues.
struct Endpoint(Host);

impl embedded_io_async::ErrorType for Endpoint {
    type Error = core::convert::Infallible;
}

impl Read for Endpoint {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            if let Some(bytes) = self.0.to_device.borrow_mut().pop_front() {
                buffer[..bytes.len()].copy_from_slice(&bytes);
                return Ok(bytes.len());
            }
            embassy_futures::yield_now().await;
        }
    }
}

impl Write for Endpoint {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        self.0.from_device.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The two ends of a mailbox placed for the test's whole run.
fn mailbox<R: Requests + 'static>() -> (Console<'static, R>, Inbox<'static, R>) {
    Box::leak(Box::new(Mailbox::<R>::new())).split()
}

/// Runs the console over `endpoint` beside `worker` until `done` holds for
/// what the host received; the run's end, if it ended first.
fn run_with<R: Requests, E: Read + Write>(
    console: &mut Console<'static, R>,
    endpoint: E,
    host: &Host,
    worker: impl core::future::Future<Output = ()>,
    done: impl Fn(&Host) -> bool,
) -> Option<Ended<E, R::Trap>> {
    let mut writer = Writer::new();
    let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
    let run = run::<Captured, R, E>(&mut writer, console, BOOT, endpoint, intake, &Chip);
    let watch = async {
        for _ in 0..10_000 {
            if done(host) {
                return;
            }
            embassy_futures::yield_now().await;
        }
        panic!("the console did not finish: {:?}", host.frames());
    };
    match block_on(select3(run, worker, watch)) {
        Either3::First(ended) => Some(ended),
        Either3::Second(()) | Either3::Third(()) => None,
    }
}

/// Runs the console beside `worker` until `done`; the run must not end.
fn run_until<R: Requests>(
    console: &mut Console<'static, R>,
    host: &Host,
    worker: impl core::future::Future<Output = ()>,
    done: impl Fn(&Host) -> bool,
) {
    if run_with(console, Endpoint(host.clone()), host, worker, done).is_some() {
        panic!("the console ended: {:?}", host.frames());
    }
}

/// A worker that never takes a request.
async fn idle() {
    core::future::pending::<()>().await;
}

/// A worker that answers each probe with its mode.
async fn prober<R: Requests>(mut inbox: Inbox<'static, R>, answer: impl Fn(R) -> ProbeMode) {
    loop {
        let (request, reply) = inbox.next().await;
        reply.answer(&ProbeArmed(answer(request)));
    }
}

fn mode(WatchdogRequest::Test(Probe(mode)): WatchdogRequest) -> ProbeMode {
    mode
}

const REJECTED: Key = oer_protocols_hil_protocol::base::Rejected::KEY;

/// The reason of every rejection the device wrote, by request.
fn rejections(host: &Host) -> Vec<(u32, RejectReason)> {
    let mut decoder = FrameDecoder::new();
    let mut found = Vec::new();
    decoder.feed(WireKind::Event, &host.from_device.borrow(), |frame| {
        let frame = frame.unwrap();
        if let Some(rejected) = frame.decode::<oer_protocols_hil_protocol::base::Rejected>() {
            found.push((frame.request_id, rejected.unwrap().body.0));
        }
    });
    found
}

#[test]
fn hello_comes_first_and_base_requests_are_answered_in_order() {
    let (mut console, _inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 5, GetBootStatus);
    host.send(0, 6, GetLinkHealth);
    run_until(&mut console, &host, idle(), |host| host.frames().len() == 3);
    let frames = host.frames();
    assert_eq!(frames[0], (0, 0, Hello::KEY));
    assert_eq!(frames[1], (5, 1, BootEvidence::KEY));
    assert_eq!(
        frames[2],
        (6, 2, oer_protocols_hil_protocol::base::LinkHealth::KEY)
    );
}

#[test]
fn a_base_answer_is_written_in_the_step_that_decoded_it_while_the_worker_holds_a_request() {
    let (mut console, mut inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 9, Probe(ProbeMode::Complete));
    host.send(0, 10, GetBootStatus);
    let gate = Cell::new(false);
    let worker = async {
        let (request, reply) = inbox.next().await;
        while !gate.get() {
            embassy_futures::yield_now().await;
        }
        reply.answer(&ProbeArmed(mode(request)));
        idle().await;
    };
    run_until(&mut console, &host, worker, |host| {
        if host.frames().len() == 2 {
            gate.set(true);
        }
        host.frames().len() == 3
    });
    assert_eq!(
        host.frames(),
        [
            (0, 0, Hello::KEY),
            (10, 1, BootEvidence::KEY),
            (9, 2, ProbeArmed::KEY)
        ]
    );
}

#[test]
fn a_reply_answers_its_own_request() {
    let (mut console, inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 9, Probe(ProbeMode::Stall));
    let worker = async move {
        let mut inbox = inbox;
        let (request, reply) = inbox.next().await;
        assert_eq!(reply.identity().request_id, 9);
        reply.answer(&ProbeArmed(mode(request)));
        idle().await;
    };
    run_until(&mut console, &host, worker, |host| host.frames().len() == 2);
    assert_eq!(host.frames()[1], (9, 1, ProbeArmed::KEY));
}

#[test]
fn a_second_request_is_refused_busy_while_one_is_in_work_and_taken_after_its_answer() {
    let (mut console, mut inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    host.send(0, 2, Probe(ProbeMode::Complete));
    let gate = Cell::new(false);
    let worker = async {
        let (request, reply) = inbox.next().await;
        while !gate.get() {
            embassy_futures::yield_now().await;
        }
        reply.answer(&ProbeArmed(mode(request)));
        let (request, reply) = inbox.next().await;
        reply.answer(&ProbeArmed(mode(request)));
        idle().await;
    };
    let sent = Cell::new(false);
    run_until(&mut console, &host, worker, |host| {
        let frames = host.frames();
        if frames.len() == 2 {
            gate.set(true);
        }
        if frames.len() == 3 && !sent.get() {
            sent.set(true);
            host.send(0, 3, Probe(ProbeMode::Complete));
        }
        frames.len() == 4
    });
    assert_eq!(
        host.frames(),
        [
            (0, 0, Hello::KEY),
            (2, 1, REJECTED),
            (1, 2, ProbeArmed::KEY),
            (3, 3, ProbeArmed::KEY)
        ]
    );
    assert_eq!(rejections(&host), [(2, RejectReason::Busy)]);
}

#[test]
fn a_rejection_names_the_request() {
    let (mut console, _inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(3, 4, Probe(ProbeMode::Complete));
    run_until(&mut console, &host, idle(), |host| host.frames().len() == 2);
    assert_eq!(host.frames()[1], (4, 1, REJECTED));
    assert_eq!(rejections(&host), [(4, RejectReason::InvalidState)]);
}

#[test]
fn a_dropped_reply_is_refused_internal_and_the_console_takes_the_next_request() {
    let (mut console, inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let worker = async move {
        let mut inbox = inbox;
        let (_request, reply) = inbox.next().await;
        drop(reply);
        prober(inbox, mode).await;
    };
    let sent = Cell::new(false);
    let mut writer_dropped = 0;
    run_until(&mut console, &host, worker, |host| {
        if host.frames().len() == 2 && !sent.get() {
            sent.set(true);
            host.send(0, 2, Probe(ProbeMode::Complete));
            host.send(0, 3, GetLinkHealth);
        }
        host.frames().len() == 4
    });
    let frames = host.frames();
    assert_eq!(frames[1], (1, 1, REJECTED));
    assert_eq!(rejections(&host), [(1, RejectReason::Internal)]);
    assert!(frames.contains(&(2, 2, ProbeArmed::KEY)) || frames.contains(&(2, 3, ProbeArmed::KEY)));
    let mut decoder = FrameDecoder::new();
    decoder.feed(WireKind::Event, &host.from_device.borrow(), |frame| {
        let frame = frame.unwrap();
        if let Some(link) = frame.decode::<oer_protocols_hil_protocol::base::LinkHealth>() {
            writer_dropped = link.unwrap().body.tx_dropped;
        }
    });
    assert_eq!(writer_dropped, 1, "the unanswered request is counted");
}

/// A response its encoder refuses.
#[derive(Debug, serde::Deserialize, postcard_schema::Schema)]
struct Refusing;

impl serde::Serialize for Refusing {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("refused"))
    }
}

impl Message for Refusing {
    const PATH: &'static str = "test/refusing";
}

#[test]
fn an_unencodable_answer_is_refused_internal_and_counted() {
    let (mut console, mut inbox) = mailbox::<WatchdogRequest>();
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let worker = async {
        let (_request, reply) = inbox.next().await;
        reply.answer(&Refusing);
        idle().await;
    };
    run_until(&mut console, &host, worker, |host| host.frames().len() == 2);
    assert_eq!(rejections(&host), [(1, RejectReason::Internal)]);
}

#[test]
fn a_trap_ends_the_run_only_after_its_answer_is_written() {
    let (mut console, mut inbox) = mailbox::<TrappingRequest>();
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let worker = async {
        let (_request, reply) = inbox.next().await;
        reply.answer_then(
            &ProbeArmed(ProbeMode::Complete),
            crate::mailbox::After::Trap(Trap),
        );
        idle().await;
    };
    let ended = run_with(&mut console, Endpoint(host.clone()), &host, worker, |_| {
        false
    });
    assert!(matches!(ended, Some(Ended::Trap(Trap))));
    assert_eq!(host.frames(), [(0, 0, Hello::KEY), (1, 1, ProbeArmed::KEY)]);
}

#[test]
fn a_trap_whose_answer_was_not_written_does_not_end_the_run() {
    /// Delivers the host's bytes and refuses every write after Hello's.
    struct Refuses {
        host: Host,
        writes: usize,
    }
    impl embedded_io_async::ErrorType for Refuses {
        type Error = embedded_io_async::ErrorKind;
    }
    impl Read for Refuses {
        async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
            Ok(Endpoint(self.host.clone()).read(buffer).await.unwrap())
        }
    }
    impl Write for Refuses {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            self.writes += 1;
            if self.writes > 2 {
                return Err(embedded_io_async::ErrorKind::Other);
            }
            self.host.from_device.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let (mut console, mut inbox) = mailbox::<TrappingRequest>();
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let answered = Cell::new(false);
    let worker = async {
        let (_request, reply) = inbox.next().await;
        reply.answer_then(
            &ProbeArmed(ProbeMode::Complete),
            crate::mailbox::After::Trap(Trap),
        );
        answered.set(true);
        idle().await;
    };
    let polls = Cell::new(0);
    let endpoint = Refuses {
        host: host.clone(),
        writes: 0,
    };
    let ended = run_with(&mut console, endpoint, &host, worker, |_| {
        if answered.get() {
            polls.set(polls.get() + 1);
        }
        polls.get() > 100
    });
    assert!(ended.is_none(), "an unwritten answer gives no trap");
    assert_eq!(host.frames(), [(0, 0, Hello::KEY)]);
}

#[test]
fn a_transport_that_takes_nothing_or_too_much_loses_the_message() {
    struct Takes(usize);
    impl embedded_io_async::ErrorType for Takes {
        type Error = core::convert::Infallible;
    }
    impl Write for Takes {
        async fn write(&mut self, _bytes: &[u8]) -> Result<usize, Self::Error> {
            Ok(self.0)
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    for taken in [0, usize::MAX] {
        let mut writer = Writer::new();
        let message = Outbound::new(0, 0, 9, &ProbeArmed(ProbeMode::Complete)).unwrap();
        assert!(!block_on(writer.write(
            &mut Takes(taken),
            BOOT,
            message,
            &Chip
        )));
        assert_eq!((writer.dropped, writer.sent_frames), (1, 0));
    }
}

fn decoded(bytes: &[u8]) -> Vec<(u32, Vec<u32>)> {
    bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.strip_suffix(b"\r").or(Some(line)))
        .filter_map(|line| {
            let mut scratch = [0; oer_portable_diagnostics::MAX_PAYLOAD + 7];
            match oer_portable_diagnostics::decode(line, &mut scratch) {
                Ok(Some((sequence, oer_portable_diagnostics::Record::Note(note)))) => {
                    Some((sequence, note.words().to_vec()))
                }
                _ => None,
            }
        })
        .collect()
}

#[test]
fn the_bytes_the_early_writer_dropped_are_reported_when_the_console_starts() {
    let (mut console, _inbox) = mailbox::<WatchdogRequest>();
    EARLY_LOST.set(37);
    let host = Host::default();
    run_until(&mut console, &host, idle(), |host| {
        decoded(&host.from_device.borrow())
            .iter()
            .any(|(_, words)| words == &[37])
    });
    assert_eq!(EARLY_LOST.get(), 0);
}

struct BrokenRead {
    length: usize,
    dropped: Rc<core::cell::Cell<bool>>,
}
impl embedded_io_async::ErrorType for BrokenRead {
    type Error = core::convert::Infallible;
}
impl Read for BrokenRead {
    async fn read(&mut self, _: &mut [u8]) -> Result<usize, Self::Error> {
        Ok(self.length)
    }
}
impl Write for BrokenRead {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        Ok(bytes.len())
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl Drop for BrokenRead {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

#[test]
fn invalid_read_length_returns_the_transport_without_releasing_it() {
    for length in [RX_CHUNK + 1, usize::MAX] {
        let (mut console, _inbox) = mailbox::<crate::base::NoRequests>();
        let mut writer = Writer::new();
        let dropped = Rc::new(Cell::new(false));
        let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
        let ended = block_on(run::<Captured, crate::base::NoRequests, _>(
            &mut writer,
            &mut console,
            BOOT,
            BrokenRead {
                length,
                dropped: dropped.clone(),
            },
            intake,
            &Chip,
        ));
        // An image without traps cannot end in one.
        let Ended::Fault(failure) = ended;
        assert_eq!(
            failure.cause,
            ConsoleFault::ReadLength {
                length,
                capacity: RX_CHUNK
            }
        );
        assert!(!dropped.get(), "the fault retains the hardware owner");
        drop(failure);
        assert!(dropped.get());
    }
}

#[test]
fn note_length_overflow_refuses_the_whole_line() {
    let mut line = NoteLine::new();
    line.length = usize::MAX;
    line.encode(&Note::diag_lost(1));
    assert!(line.framed().is_empty());
    assert_eq!(line.bytes, [0; MAX_NOTE_LINE + 1]);
}

#[test]
fn boot_evidence_acknowledges_retained_data_only_after_frame_and_trailer_succeed() {
    use oer_portable_diagnostics::fault::{Class, Domain, Record};
    struct Observed {
        retained: RefCell<Record>,
    }
    impl Platform for Observed {
        fn boot_evidence(&self) -> BootEvidence {
            Chip.boot_evidence()
        }
        fn boot_evidence_written(&self) {
            self.retained
                .borrow_mut()
                .report(|_| Ok::<_, ()>(()))
                .unwrap();
        }
        fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
            Chip.post_mortem_checkpoints(first)
        }
    }
    struct Output {
        refused: usize,
        calls: usize,
    }
    impl embedded_io_async::ErrorType for Output {
        type Error = embedded_io_async::ErrorKind;
    }
    impl Write for Output {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            self.calls += 1;
            if self.calls == self.refused {
                Err(embedded_io_async::ErrorKind::Other)
            } else {
                Ok(bytes.len())
            }
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    for refused in [1, 2, 3] {
        let retained =
            Record::new(Class::RustPanic, Domain::Postmortem, 2, 0, 0, &[7; 44]).unwrap();
        let platform = Observed {
            retained: RefCell::new(retained),
        };
        let mut writer = Writer::new();
        let message = Outbound::new(1, 0, 1, &platform.boot_evidence()).unwrap();
        let _written =
            block_on(writer.write(&mut Output { refused, calls: 0 }, BOOT, message, &platform));
        if refused <= 2 {
            assert_eq!(*platform.retained.borrow(), retained);
        } else {
            assert!(platform.retained.borrow().peek().is_err());
        }
    }
}

crate::service! {
    /// A test service with one handled endpoint.
    enum ProbeService (sessions = false, handler = ProbeHandler) {
        Probe(Probe) => probe,
    }
}

/// Answers a probe with its mode and counts the calls.
struct Prober {
    calls: u32,
}

impl ProbeHandler for Prober {
    async fn probe(&mut self, Probe(mode): Probe) -> ProbeArmed {
        self.calls += 1;
        ProbeArmed(mode)
    }
}

#[test]
fn a_service_answers_each_request_through_its_handler_and_its_reply() {
    let (mut console, mut inbox) = mailbox::<ProbeService>();
    let host = Host::default();
    host.send(0, 21, Probe(ProbeMode::Stall));
    let calls = Cell::new(0);
    let worker = async {
        let mut prober = Prober { calls: 0 };
        let (request, reply) = inbox.next().await;
        request.serve(&mut prober, reply).await;
        calls.set(prober.calls);
        idle().await;
    };
    run_until(&mut console, &host, worker, |host| host.frames().len() == 2);
    assert_eq!(calls.get(), 1);
    assert_eq!(host.frames()[1], (21, 1, ProbeArmed::KEY));
    assert_eq!(
        <ProbeService as crate::base::Service>::REQUESTS,
        [Probe::KEY]
    );
}
