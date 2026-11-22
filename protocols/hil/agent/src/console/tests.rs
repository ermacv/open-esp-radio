use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use core::pin::pin;
use core::task::{Context, Poll, Waker};

use crate::test_messages::{Probe, ProbeArmed, ProbeMode};
use embassy_futures::block_on;
use embassy_futures::select::{Either, select};
use oer_protocols_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetImageKeys, GetLinkHealth, Hello, ImageKeySet,
    PostMortemCheckpoints, RejectReason, ResetReason, sorted_keys,
};
use oer_protocols_hil_protocol::{Envelope, FrameDecoder, Key, WireKind};

use super::*;

crate::requests! {
    enum WatchdogRequest (sessions = false) {
        Test(Probe),
    }
}

const BOOT: u64 = 77;
const DEPTH: usize = 8;

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

/// `K` lanes of the test's depth, placed for the test's whole run.
fn lanes<const K: usize>() -> [Ends<'static, DEPTH>; K] {
    Box::leak(Box::new(Lanes::<K, DEPTH>::new())).split()
}

/// The ends apart: the producers and the writer's drains.
fn apart<const K: usize>(
    ends: [Ends<'static, DEPTH>; K],
) -> (
    Vec<Producer<'static, DEPTH>>,
    [LaneDrain<'static, DEPTH>; K],
) {
    let mut producers = Vec::new();
    let drains = ends.map(|Ends { producer, drain }| {
        producers.push(producer);
        drain
    });
    (producers, drains)
}

/// Runs the writer until `done` holds for what the host received.
fn run_until<const K: usize>(
    lanes: &mut [LaneDrain<'static, DEPTH>; K],
    host: &Host,
    serve: impl FnMut(RequestIdentity, WatchdogRequest) -> Result<(), Busy>,
    done: impl Fn(&Host) -> bool,
) {
    let mut writer = Writer::new();
    let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
    let run = run::<Captured, _, _, K, DEPTH>(
        &mut writer,
        lanes,
        BOOT,
        Endpoint(host.clone()),
        intake,
        &Chip,
        serve,
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
        Either::First(failure) => panic!("console failed: {:?}", failure.cause),
        Either::Second(()) => {}
    }
}

fn poll<F: core::future::Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn hello_comes_first_and_base_requests_are_answered_in_order() {
    let (_, mut drains) = apart(lanes::<1>());
    let host = Host::default();
    host.send(0, 5, GetBootStatus);
    host.send(0, 6, GetLinkHealth);
    run_until(
        &mut drains,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 3,
    );
    let frames = host.frames();
    assert_eq!(frames[0], (0, 0, Hello::KEY));
    assert_eq!(frames[1], (5, 1, BootEvidence::KEY));
    assert_eq!(
        frames[2],
        (6, 2, oer_protocols_hil_protocol::base::LinkHealth::KEY)
    );
}

#[test]
fn the_image_serves_its_own_requests_and_its_producer_s_reply_is_written() {
    let (mut producers, mut drains) = apart(lanes::<1>());
    let host = Host::default();
    host.send(0, 9, Probe(ProbeMode::Complete));
    let mut producer = producers.remove(0);
    run_until(
        &mut drains,
        &host,
        |request, WatchdogRequest::Test(Probe(mode))| {
            assert_eq!(request.request_id, 9);
            producer
                .publish(0, request.request_id, &ProbeArmed(mode))
                .expect("room for the reply");
            Ok(())
        },
        |host| host.frames().len() == 2,
    );
    assert_eq!(host.frames()[1], (9, 1, ProbeArmed::KEY));
}

#[test]
fn a_busy_hand_over_is_answered_rejected_by_the_writer() {
    let (_, mut drains) = apart(lanes::<1>());
    let host = Host::default();
    host.send(0, 4, Probe(ProbeMode::Complete));
    run_until(
        &mut drains,
        &host,
        |_, _| Err(Busy),
        |host| host.frames().len() == 2,
    );
    let (request, sequence, key) = host.frames()[1];
    assert_eq!(
        (request, sequence, key),
        (4, 1, oer_protocols_hil_protocol::base::Rejected::KEY)
    );
}

#[test]
fn a_rejection_names_the_request() {
    let (_, mut drains) = apart(lanes::<1>());
    let host = Host::default();
    host.send(3, 4, Probe(ProbeMode::Complete));
    run_until(
        &mut drains,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 2,
    );
    let (request, _, key) = host.frames()[1];
    assert_eq!(
        (request, key),
        (4, oer_protocols_hil_protocol::base::Rejected::KEY)
    );
    let _ = RejectReason::InvalidState;
}

#[test]
fn hello_leads_the_boot_although_the_image_published_first() {
    let (mut producers, mut drains) = apart(lanes::<1>());
    let host = Host::default();
    assert_eq!(
        producers[0].publish(0, 0, &ProbeArmed(ProbeMode::Complete)),
        Ok(())
    );
    run_until(
        &mut drains,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 2,
    );
    assert_eq!(host.frames(), [(0, 0, Hello::KEY), (0, 1, ProbeArmed::KEY)]);
}

#[test]
fn a_full_lane_drops_and_counts_its_own_message_alone() {
    let (mut producers, drains) = apart(lanes::<2>());
    for request in 0..DEPTH as u32 {
        assert_eq!(producers[0].publish(0, request, &GetBootStatus), Ok(()));
    }
    assert_eq!(producers[0].publish(0, 8, &GetBootStatus), Err(Dropped));
    assert_eq!(
        producers[1].publish(0, 9, &GetBootStatus),
        Ok(()),
        "another lane has its own room"
    );
    let writer = Writer::new();
    assert_eq!(writer.sent(&drains).dropped, 1);
}

#[test]
fn the_writer_numbers_in_its_order_own_answers_first_then_each_lane_in_turn() {
    const EACH: u32 = 3;
    // Every order of the two lanes' producers: the lanes' FIFOs keep each
    // context's order, and the writer drains lane 0 before lane 1.
    for order in 0_u32..(1 << (2 * EACH)) {
        if order.count_ones() != EACH {
            continue;
        }
        let (mut producers, mut drains) = apart(lanes::<2>());
        let (mut first, mut second) = (0, 0);
        for turn in 0..2 * EACH {
            let message = ProbeArmed(ProbeMode::Complete);
            if order & (1 << turn) != 0 {
                producers[1]
                    .publish(0, 200 + second, &message)
                    .expect("room");
                second += 1;
            } else {
                producers[0]
                    .publish(0, 100 + first, &message)
                    .expect("room");
                first += 1;
            }
        }
        let host = Host::default();
        run_until(
            &mut drains,
            &host,
            |_, _| unreachable!(),
            |host| host.frames().len() == 1 + 2 * EACH as usize,
        );
        let frames = host.frames();
        let numbers: Vec<u32> = frames.iter().map(|(_, sequence, _)| *sequence).collect();
        assert_eq!(
            numbers,
            (0..=2 * EACH).collect::<Vec<_>>(),
            "numbered as written"
        );
        let requests: Vec<u32> = frames
            .iter()
            .skip(1)
            .map(|(request, ..)| *request)
            .collect();
        assert_eq!(
            requests,
            [100, 101, 102, 200, 201, 202],
            "order {order:06b}: lane 0's batch, then lane 1's"
        );
    }
}

#[test]
fn a_reliable_producer_waits_for_its_lane_s_room() {
    let (mut producers, drains) = apart(lanes::<1>());
    let mut producer = producers.remove(0);
    for request in 0..DEPTH as u32 {
        producer.publish(0, request, &GetBootStatus).expect("room");
    }
    {
        let mut waiting = pin!(producer.publish_reliably(0, 50, &ProbeArmed(ProbeMode::Complete)));
        assert!(poll(waiting.as_mut()).is_pending(), "the lane is full");
        assert!(
            drains[0].receiver.try_receive().is_some(),
            "the writer takes one"
        );
        assert_eq!(
            poll(waiting.as_mut()),
            Poll::Ready(Ok(Entered(DEPTH as u32)))
        );
    }
}

#[test]
fn a_waiter_is_released_by_its_own_message_s_write_and_not_before() {
    let (mut producers, mut drains) = apart(lanes::<1>());
    let mut producer = producers.remove(0);
    let first =
        block_on(producer.publish_reliably(0, 1, &ProbeArmed(ProbeMode::Complete))).unwrap();
    let second =
        block_on(producer.publish_reliably(0, 2, &ProbeArmed(ProbeMode::Complete))).unwrap();
    assert_eq!((first, second), (Entered(0), Entered(1)));
    let mut writer = Writer::new();
    let host = Host::default();
    let message = drains[0].receiver.try_receive().unwrap();
    block_on(writer.write_lane(
        &mut Endpoint(host.clone()),
        BOOT,
        &mut drains[0],
        message,
        &Chip,
    ));
    {
        let mut waiting = pin!(producer.written(second));
        assert!(
            poll(waiting.as_mut()).is_pending(),
            "the first written, not the second"
        );
        let message = drains[0].receiver.try_receive().unwrap();
        block_on(writer.write_lane(
            &mut Endpoint(host.clone()),
            BOOT,
            &mut drains[0],
            message,
            &Chip,
        ));
        assert_eq!(poll(waiting.as_mut()), Poll::Ready(()));
    }
    assert_eq!(
        poll(pin!(producer.written(first))),
        Poll::Ready(()),
        "a wait after the write returns at once"
    );
}

#[test]
fn a_control_reply_is_not_written_until_the_entire_frame_and_trailer_reach_the_endpoint() {
    /// Where the split transport is: its first write not taken yet, a half
    /// taken and the gate shut, or the gate opened.
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Step {
        Whole,
        Split,
        Open,
    }

    struct SplitTx<'a> {
        host: Host,
        gate: &'a Cell<bool>,
        step: Step,
    }
    impl embedded_io_async::ErrorType for SplitTx<'_> {
        type Error = core::convert::Infallible;
    }
    impl Write for SplitTx<'_> {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            if self.step == Step::Whole {
                self.step = Step::Split;
                let count = bytes.len() / 2;
                self.host
                    .from_device
                    .borrow_mut()
                    .extend_from_slice(&bytes[..count]);
                return Ok(count);
            }
            if self.step == Step::Split {
                while !self.gate.get() {
                    embassy_futures::yield_now().await;
                }
                self.step = Step::Open;
            }
            self.host.from_device.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let (mut producers, mut drains) = apart(lanes::<1>());
    let mut producer = producers.remove(0);
    let entered = block_on(producer.publish_reliably(0, 9, &ProbeArmed(ProbeMode::Complete)))
        .expect("the reply is built");
    let message = drains[0].receiver.try_receive().expect("queued reply");
    let host = Host::default();
    let gate = Cell::new(false);
    let mut tx = SplitTx {
        host: host.clone(),
        gate: &gate,
        step: Step::Whole,
    };
    let mut writer = Writer::new();
    let (drain, _) = drains.split_at_mut(1);
    let mut writing = pin!(writer.write_lane(&mut tx, BOOT, &mut drain[0], message, &Chip));
    let mut written = pin!(producer.written(entered));
    assert_eq!(poll(written.as_mut()), Poll::Pending);
    assert_eq!(poll(writing.as_mut()), Poll::Pending);
    assert!(!host.from_device.borrow().is_empty());
    assert!(host.frames().is_empty());
    assert_eq!(poll(written.as_mut()), Poll::Pending);
    gate.set(true);
    assert_eq!(poll(writing.as_mut()), Poll::Ready(()));
    assert_eq!(poll(written.as_mut()), Poll::Ready(()));
    assert_eq!(host.frames(), [(9, 0, ProbeArmed::KEY)]);
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

#[test]
fn reset_acknowledgement_requires_the_complete_write_and_keeps_correlation() {
    use oer_protocols_hil_protocol::lifecycle::ResetAccepted;

    let (mut producers, mut drains) = apart(lanes::<1>());
    let mut producer = producers.remove(0);
    let request = RequestIdentity {
        boot_id: BOOT,
        request_id: u32::MAX,
        session_id: 0x89ab_cdef_0123_4567,
    };
    let mut acknowledging = pin!(crate::lifecycle::acknowledge_reset(&mut producer, request));
    assert!(poll(acknowledging.as_mut()).is_pending());
    let message = drains[0].receiver.try_receive().unwrap();
    let host = Host::default();
    assert!(host.frames().is_empty());
    let mut writer = Writer::new();
    block_on(writer.write_lane(
        &mut Endpoint(host.clone()),
        BOOT,
        &mut drains[0],
        message,
        &Chip,
    ));
    let Poll::Ready(Ok(acknowledged)) = poll(acknowledging.as_mut()) else {
        panic!("a complete write releases the reset acknowledgement");
    };
    assert_eq!(host.frames(), [(u32::MAX, 0, ResetAccepted::KEY)]);
    assert_eq!(
        acknowledged.observations(),
        [u32::MAX, 0, 0x0123_4567, 0x89ab_cdef]
    );
}

/// The notes a stream of console lines carries, in order: the sequence each
/// was numbered with and its words.
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
    let (_, mut drains) = apart(lanes::<1>());
    EARLY_LOST.set(37);
    let host = Host::default();
    run_until(
        &mut drains,
        &host,
        |_, _| Ok(()),
        |host| {
            decoded(&host.from_device.borrow())
                .iter()
                .any(|(_, words)| words == &[37])
        },
    );
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
        let (_, mut drains) = apart(lanes::<1>());
        let mut writer = Writer::new();
        let dropped = Rc::new(Cell::new(false));
        let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0, 0);
        let failure = block_on(run::<Captured, crate::base::NoRequests, _, 1, DEPTH>(
            &mut writer,
            &mut drains,
            BOOT,
            BrokenRead {
                length,
                dropped: dropped.clone(),
            },
            intake,
            &Chip,
            |_, _| Ok(()),
        ));
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
fn a_service_answers_each_request_through_its_handler_with_the_request_identity() {
    let (mut producers, mut drains) = apart(lanes::<1>());
    let mut producer = producers.remove(0);
    let mut prober = Prober { calls: 0 };
    let identity = oer_protocols_hil_protocol::RequestIdentity {
        boot_id: BOOT,
        session_id: 0,
        request_id: 21,
    };
    block_on(ProbeService::Probe(Probe(ProbeMode::Stall)).serve(
        &mut prober,
        &mut producer,
        identity,
    ))
    .expect("the response is built");
    assert_eq!(prober.calls, 1);
    let message = drains[0]
        .receiver
        .try_receive()
        .expect("the queued response");
    let host = Host::default();
    let mut writer = Writer::new();
    block_on(writer.write_lane(
        &mut Endpoint(host.clone()),
        BOOT,
        &mut drains[0],
        message,
        &Chip,
    ));
    assert_eq!(host.frames(), [(21, 0, ProbeArmed::KEY)]);
    assert_eq!(
        <ProbeService as crate::base::Service>::REQUESTS,
        [Probe::KEY]
    );
}
