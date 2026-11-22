use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use crate::serve::{Ran, Trap, Unanswered};
use crate::test_messages::{Probe, ProbeArmed, ProbeMode, Switch, Switched, Trigger, Triggered};
use embassy_futures::block_on;
use embassy_futures::select::{Either as Race, select};
use oer_protocols_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetImageKeys, GetLinkHealth, Hello, ImageKeySet,
    PostMortemCheckpoints, RejectReason, ResetReason, sorted_keys,
};
use oer_protocols_hil_protocol::{Envelope, FrameDecoder, Key, Message, WireKind};

use super::*;

crate::service! {
    /// A test service: an observation and a command.
    enum Probes (sessions = false, observer = ProbeObserver, commands = ProbeCommand, commander = ProbeCommander) {
        Switch(Switch) => observe switch,
        Probe(Probe) => command probe,
    }
}

crate::service! {
    /// A set whose command may end in a trap, as the fault service's.
    enum Trapping (sessions = false, observer = TrapObserver, commands = TrapCommand, commander = TrapCommander, traps = true) {
        Probe(Probe) => command probe,
    }
}

crate::service! {
    /// A set whose commands the image runs itself.
    enum Forwarded (sessions = false, observer = ForwardObserver, commands = ForwardCommand) {
        Probe(Probe) => command probe,
    }
}

crate::service! {
    /// A second service, composed beside the first.
    enum Others (sessions = false, observer = OtherObserver, commands = OtherCommand, commander = OtherCommander) {
        Trigger(Trigger) => command trigger,
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

const SERVED: [Key; 8] = sorted_keys([
    GetImageKeys::KEY,
    GetBootStatus::KEY,
    GetLinkHealth::KEY,
    Hello::KEY,
    Probe::KEY,
    ProbeArmed::KEY,
    Switch::KEY,
    Switched::KEY,
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

/// Answers observations: counts them.
#[derive(Default)]
struct Observing {
    switches: Cell<u32>,
}

impl ProbeObserver for Observing {
    fn switch(&mut self, _request: Switch) -> Switched {
        self.switches.set(self.switches.get() + 1);
        Switched
    }
}

impl TrapObserver for Observing {}
impl ForwardObserver for Observing {}
impl OtherObserver for Observing {}

/// Answers commands once its gate opens; a gate that never opens is a
/// command that does not come back.
struct Commanding<'a> {
    gate: &'a Cell<bool>,
    started: &'a Cell<u32>,
}

async fn through(gate: &Cell<bool>) {
    while !gate.get() {
        embassy_futures::yield_now().await;
    }
}

impl ProbeCommander for Commanding<'_> {
    async fn probe(&mut self, Probe(mode): Probe) -> ProbeArmed {
        self.started.set(self.started.get() + 1);
        through(self.gate).await;
        ProbeArmed(mode)
    }
}

impl TrapCommander for Commanding<'_> {
    async fn probe(&mut self, Probe(mode): Probe) -> (ProbeArmed, After<Trap>) {
        through(self.gate).await;
        (ProbeArmed(mode), After::Trap(Trap))
    }
}

impl OtherCommander for Commanding<'_> {
    async fn trigger(&mut self, _request: Trigger) -> Triggered {
        Triggered
    }
}

/// The image's own run of its commands: one that gives no answer.
struct Silent;

impl Run<Silent> for ForwardCommand {
    type Answer = core::convert::Infallible;
    type Trap = core::convert::Infallible;
    async fn run(self, _commander: &mut Silent) -> Ran<Self::Answer, Self::Trap> {
        let ForwardCommand::Probe(Probe(_mode)) = self;
        Err(Unanswered)
    }
}

/// Runs the console over `endpoint` for the service set `R` until `done`
/// holds for what the host received; the run's end, if it ended first.
fn run_with<R, O, C, E: Read + Write>(
    observer: &mut O,
    commander: &mut C,
    endpoint: E,
    host: &Host,
    done: impl Fn(&Host) -> bool,
) -> Option<Ended<E, <R::Command as Run<C>>::Trap>>
where
    R: Serve<O>,
    R::Command: Run<C>,
{
    let mut writer = Writer::new();
    let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
    let run = run::<Captured, R, O, C, E>(
        &mut writer,
        observer,
        commander,
        BOOT,
        endpoint,
        intake,
        &Chip,
    );
    let watch = async {
        for _ in 0..10_000 {
            if done(host) {
                return;
            }
            embassy_futures::yield_now().await;
        }
        panic!("the console did not finish: {:?}", host.frames());
    };
    match block_on(select(run, watch)) {
        Race::First(ended) => Some(ended),
        Race::Second(()) => None,
    }
}

/// Runs the console until `done`; the run must not end.
fn run_until<R, O, C>(
    observer: &mut O,
    commander: &mut C,
    host: &Host,
    done: impl Fn(&Host) -> bool,
) where
    R: Serve<O>,
    R::Command: Run<C>,
{
    if run_with::<R, O, C, _>(observer, commander, Endpoint(host.clone()), host, done).is_some() {
        panic!("the console ended: {:?}", host.frames());
    }
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
    let host = Host::default();
    host.send(0, 5, GetBootStatus);
    host.send(0, 6, GetLinkHealth);
    run_until::<crate::base::NoRequests, _, _>(&mut (), &mut (), &host, |host| {
        host.frames().len() == 3
    });
    let frames = host.frames();
    assert_eq!(frames[0], (0, 0, Hello::KEY));
    assert_eq!(frames[1], (5, 1, BootEvidence::KEY));
    assert_eq!(
        frames[2],
        (6, 2, oer_protocols_hil_protocol::base::LinkHealth::KEY)
    );
}

#[test]
fn an_observation_is_answered_in_the_step_that_decoded_it() {
    let host = Host::default();
    host.send(0, 3, Switch);
    let (gate, started) = (Cell::new(true), Cell::new(0));
    let mut observing = Observing::default();
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    run_until::<Probes, _, _>(&mut observing, &mut commanding, &host, |host| {
        host.frames().len() == 2
    });
    assert_eq!(host.frames()[1], (3, 1, Switched::KEY));
    assert_eq!((observing.switches.get(), started.get()), (1, 0));
}

#[test]
fn a_command_is_answered_when_it_comes_back_and_the_base_module_meanwhile() {
    let host = Host::default();
    host.send(0, 9, Probe(ProbeMode::Stall));
    host.send(0, 10, GetBootStatus);
    let (gate, started) = (Cell::new(false), Cell::new(0));
    let mut observing = Observing::default();
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    run_until::<Probes, _, _>(&mut observing, &mut commanding, &host, |host| {
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
fn while_a_command_does_not_come_back_an_observation_is_answered_a_second_command_is_busy_and_the_base_module_answers()
 {
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Stall));
    let (gate, started) = (Cell::new(false), Cell::new(0));
    let mut observing = Observing::default();
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    let sent = Cell::new(false);
    run_until::<Probes, _, _>(&mut observing, &mut commanding, &host, |host| {
        if started.get() == 1 && !sent.get() {
            sent.set(true);
            host.send(0, 2, Switch);
            host.send(0, 3, Probe(ProbeMode::Complete));
            host.send(0, 4, GetBootStatus);
        }
        host.frames().len() == 4
    });
    assert_eq!(
        host.frames(),
        [
            (0, 0, Hello::KEY),
            (2, 1, Switched::KEY),
            (3, 2, REJECTED),
            (4, 3, BootEvidence::KEY)
        ]
    );
    assert_eq!(rejections(&host), [(3, RejectReason::Busy)]);
    assert_eq!(started.get(), 1, "the second command never ran");
    assert!(!gate.get(), "the first command is still in work");
}

#[test]
fn a_rejection_names_the_request() {
    let host = Host::default();
    host.send(3, 4, Probe(ProbeMode::Complete));
    let (gate, started) = (Cell::new(true), Cell::new(0));
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    run_until::<Probes, _, _>(&mut Observing::default(), &mut commanding, &host, |host| {
        host.frames().len() == 2
    });
    assert_eq!(host.frames()[1], (4, 1, REJECTED));
    assert_eq!(rejections(&host), [(4, RejectReason::InvalidState)]);
}

#[test]
fn a_command_that_gives_no_answer_is_refused_internal_and_counted() {
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let sent = Cell::new(false);
    run_until::<Forwarded, _, _>(&mut Observing::default(), &mut Silent, &host, |host| {
        if host.frames().len() == 2 && !sent.get() {
            sent.set(true);
            host.send(0, 2, GetLinkHealth);
        }
        host.frames().len() == 3
    });
    assert_eq!(rejections(&host), [(1, RejectReason::Internal)]);
    let mut dropped = None;
    let mut decoder = FrameDecoder::new();
    decoder.feed(WireKind::Event, &host.from_device.borrow(), |frame| {
        if let Some(link) = frame
            .unwrap()
            .decode::<oer_protocols_hil_protocol::base::LinkHealth>()
        {
            dropped = Some(link.unwrap().body.tx_dropped);
        }
    });
    assert_eq!(dropped, Some(1), "the unanswered command is counted");
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
    // An answer the encoder refuses: a frame too long for its buffer.
    let host = Host::default();
    let mut writer = Writer::new();
    let written = writer
        .frames
        .frame(
            BOOT,
            RequestIdentity {
                boot_id: BOOT,
                session_id: 0,
                request_id: 1,
            },
        )
        .write(&Refusing);
    let delivery = block_on(send(
        &mut writer.counts,
        &mut Endpoint(host.clone()),
        written,
    ));
    assert!(matches!(delivery, Delivery::Unencoded));
    assert_eq!(writer.counts.dropped, 1);
    assert!(host.frames().is_empty());
}

#[test]
fn a_trap_ends_the_run_only_after_its_answer_is_written() {
    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let (gate, started) = (Cell::new(true), Cell::new(0));
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    let ended = run_with::<Trapping, _, _, _>(
        &mut Observing::default(),
        &mut commanding,
        Endpoint(host.clone()),
        &host,
        |_| false,
    );
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

    let host = Host::default();
    host.send(0, 1, Probe(ProbeMode::Complete));
    let (gate, started) = (Cell::new(true), Cell::new(0));
    let mut commanding = Commanding {
        gate: &gate,
        started: &started,
    };
    let polls = Cell::new(0);
    let endpoint = Refuses {
        host: host.clone(),
        writes: 0,
    };
    let ended = run_with::<Trapping, _, _, _>(
        &mut Observing::default(),
        &mut commanding,
        endpoint,
        &host,
        |_| {
            polls.set(polls.get() + 1);
            polls.get() > 200
        },
    );
    assert!(ended.is_none(), "an unwritten answer gives no trap");
    assert_eq!(host.frames(), [(0, 0, Hello::KEY)]);
}

#[test]
fn two_services_composed_answer_each_its_own_requests() {
    let host = Host::default();
    host.send(0, 1, Trigger);
    let sent = Cell::new(false);
    let (gate, started) = (Cell::new(true), Cell::new(0));
    let mut observers = (Observing::default(), Observing::default());
    let mut commanders = (
        Commanding {
            gate: &gate,
            started: &started,
        },
        Commanding {
            gate: &gate,
            started: &started,
        },
    );
    run_until::<crate::base::Either<Probes, Others>, _, _>(
        &mut observers,
        &mut commanders,
        &host,
        |host| {
            if host.frames().len() == 2 && !sent.get() {
                sent.set(true);
                host.send(0, 2, Switch);
                host.send(0, 3, Probe(ProbeMode::Complete));
            }
            host.frames().len() == 4
        },
    );
    assert_eq!(
        host.frames(),
        [
            (0, 0, Hello::KEY),
            (1, 1, Triggered::KEY),
            (2, 2, Switched::KEY),
            (3, 3, ProbeArmed::KEY)
        ]
    );
    assert_eq!(observers.0.switches.get(), 1);
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
        let identity = RequestIdentity {
            boot_id: BOOT,
            session_id: 0,
            request_id: 9,
        };
        let written = writer
            .frames
            .frame(BOOT, identity)
            .write(&ProbeArmed(ProbeMode::Complete));
        let delivery = block_on(send(&mut writer.counts, &mut Takes(taken), written));
        assert!(matches!(delivery, Delivery::Broken));
        assert_eq!((writer.counts.dropped, writer.counts.sent_frames), (1, 0));
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
    EARLY_LOST.set(37);
    let host = Host::default();
    run_until::<crate::base::NoRequests, _, _>(&mut (), &mut (), &host, |host| {
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
        let mut writer = Writer::new();
        let dropped = Rc::new(Cell::new(false));
        let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
        let ended = block_on(run::<Captured, crate::base::NoRequests, (), (), _>(
            &mut writer,
            &mut (),
            &mut (),
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
    let mut encoder = oer_protocols_hil_protocol::FrameEncoder::new();
    let request = encoder
        .encode(&Envelope::new(BOOT, 0, 0, 1, GetBootStatus))
        .unwrap()
        .to_vec();
    for refused in [1, 2, 3] {
        let retained =
            Record::new(Class::RustPanic, Domain::Postmortem, 2, 0, 0, &[7; 44]).unwrap();
        let platform = Observed {
            retained: RefCell::new(retained),
        };
        let mut writer = Writer::new();
        let mut intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
        let base = Base {
            boot: BOOT,
            image_keys: intake.image_keys(),
            platform: &platform,
        };
        let _fed = block_on(feed::<_, crate::base::NoRequests, (), _>(
            &mut writer,
            &mut (),
            &mut Output { refused, calls: 0 },
            &mut intake,
            &base,
            &request,
            Work::Idle,
        ));
        if refused <= 2 {
            assert_eq!(*platform.retained.borrow(), retained);
        } else {
            assert!(platform.retained.borrow().peek().is_err());
        }
    }
}
