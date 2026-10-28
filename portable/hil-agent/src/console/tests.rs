use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use embassy_futures::block_on;
use embassy_futures::select::{Either, select};
use oer_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetImageKeys, GetLinkHealth, Hello, ImageKeySet,
    PostMortemCheckpoints, RejectReason, ResetReason, sorted_keys,
};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use oer_hil_protocol::{Envelope, FrameDecoder, Key, WireKind};

use super::*;
use oer_diagnostics::Note;

const BOOT: u64 = 77;

/// The early writer of a test thread: it keeps the words of the notes it
/// is given while it owns the peripheral.
struct Captured;

impl Immediate for Captured {
    fn note(note: &Note<'_>) -> bool {
        if !EARLY_OWNS.get() {
            return false;
        }
        EARLY.with(|notes| notes.borrow_mut().push(note.words().to_vec()));
        true
    }

    fn take_lost() -> u32 {
        EARLY_LOST.replace(0)
    }
}

type TestConsole = Console<4, 8, Captured>;

fn console() -> &'static TestConsole {
    Box::leak(Box::new(TestConsole::new()))
}

std::thread_local! {
    static EARLY: RefCell<Vec<Vec<u32>>> = const { RefCell::new(Vec::new()) };
    static EARLY_OWNS: core::cell::Cell<bool> = const { core::cell::Cell::new(true) };
    static EARLY_LOST: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

struct Chip;

impl Platform for Chip {
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
    WatchdogTest::KEY,
    WatchdogArmed::KEY,
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

/// Runs the console until `done` holds for what the host received.
fn run_until(
    console: &'static TestConsole,
    host: &Host,
    mut served: impl FnMut(RequestIdentity, WatchdogTest),
    done: impl Fn(&Host) -> bool,
) {
    let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED), 0);
    let run = console.run(
        Endpoint(host.clone()),
        intake,
        &Chip,
        |request, crate::base::WatchdogRequest::Test(own)| served(request, own),
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

#[test]
fn hello_comes_first_and_base_requests_are_answered_in_order() {
    let console = console();
    let host = Host::default();
    console.start(BOOT);
    host.send(0, 5, GetBootStatus);
    host.send(0, 6, GetLinkHealth);
    run_until(
        console,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 3,
    );
    let frames = host.frames();
    assert_eq!(frames[0], (0, 0, Hello::KEY));
    assert_eq!(frames[1], (5, 1, BootEvidence::KEY));
    assert_eq!(frames[2], (6, 2, oer_hil_protocol::base::LinkHealth::KEY));
}

#[test]
fn the_image_serves_its_own_requests_and_publishes_their_replies() {
    let console = console();
    let host = Host::default();
    console.start(BOOT);
    host.send(0, 9, WatchdogTest(WatchdogTestMode::Complete));
    run_until(
        console,
        &host,
        |request, WatchdogTest(mode)| {
            assert_eq!(request.request_id, 9);
            console.publish(0, request.request_id, &WatchdogArmed(mode));
        },
        |host| host.frames().len() == 2,
    );
    assert_eq!(host.frames()[1], (9, 1, WatchdogArmed::KEY));
}

#[test]
fn a_rejection_names_the_request() {
    let console = console();
    let host = Host::default();
    console.start(BOOT);
    host.send(3, 4, WatchdogTest(WatchdogTestMode::Complete));
    run_until(
        console,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 2,
    );
    let (request, _, key) = host.frames()[1];
    assert_eq!((request, key), (4, oer_hil_protocol::base::Rejected::KEY));
    let _ = RejectReason::InvalidState;
}

#[test]
fn hello_leads_the_boot_although_the_image_published_first() {
    let console = console();
    let host = Host::default();
    console.start(BOOT);
    assert_eq!(
        console.publish(0, 0, &WatchdogArmed(WatchdogTestMode::Complete)),
        Some(1)
    );
    run_until(
        console,
        &host,
        |_, _| unreachable!(),
        |host| host.frames().len() == 2,
    );
    assert_eq!(
        host.frames(),
        [(0, 0, Hello::KEY), (0, 1, WatchdogArmed::KEY)]
    );
}

#[test]
fn a_full_queue_drops_and_counts_instead_of_blocking() {
    let console = console();
    console.start(BOOT);
    for request in 0..8 {
        assert_eq!(
            console.publish(0, request, &GetBootStatus),
            Some(request + 1)
        );
    }
    assert_eq!(console.publish(0, 8, &GetBootStatus), None);
    assert_eq!(console.link_health().unwrap().tx_dropped, 1);
}

fn rearm(step: u32) -> Note<'static> {
    Note::rearm(step, 20, 20_000)
}

/// The notes a stream of console lines carries, in order: the sequence each
/// was numbered with and its words.
fn decoded(bytes: &[u8]) -> Vec<(u32, Vec<u32>)> {
    bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.strip_suffix(b"\r").or(Some(line)))
        .filter_map(|line| {
            let mut scratch = [0; oer_diagnostics::MAX_PAYLOAD + 7];
            match oer_diagnostics::decode(line, &mut scratch) {
                Ok(Some((sequence, oer_diagnostics::Record::Note(note)))) => {
                    Some((sequence, note.words().to_vec()))
                }
                _ => None,
            }
        })
        .collect()
}

#[test]
fn a_note_before_the_transport_owns_the_peripheral_goes_to_the_early_writer() {
    let console = console();
    EARLY.with(|notes| notes.borrow_mut().clear());
    console.note_immediately(&rearm(1));
    block_on(console.note_reliably(&rearm(2)));
    EARLY.with(|notes| assert_eq!(notes.borrow().len(), 2));
    assert!(console.notes.try_receive().is_err(), "nothing was queued");
    assert_eq!(console.dropped_notes(), 0);
}

#[test]
fn a_note_after_the_early_writer_gave_up_the_peripheral_is_queued() {
    let console = console();
    EARLY.with(|notes| notes.borrow_mut().clear());
    EARLY_OWNS.set(false);
    console.note_immediately(&rearm(1));
    block_on(console.note_reliably(&rearm(2)));
    EARLY_OWNS.set(true);
    EARLY.with(|notes| assert!(notes.borrow().is_empty()));
    assert_eq!(console.notes.try_receive().expect("queued").words()[0], 1);
    assert_eq!(console.notes.try_receive().expect("queued").words()[0], 2);
}

#[test]
fn an_immediate_note_is_dropped_and_counted_when_the_queue_is_full() {
    let console = console();
    console.running.store(true, Ordering::Release);
    for step in 0..4 {
        console.note_immediately(&rearm(step));
    }
    assert_eq!(console.dropped_notes(), 0);
    console.note_immediately(&rearm(9));
    assert_eq!(console.dropped_notes(), 1);
    assert_eq!(console.link_health().unwrap().notes_dropped, 1);
}

#[test]
fn the_bytes_the_early_writer_dropped_are_reported_when_the_console_starts() {
    let console = console();
    EARLY_LOST.set(37);
    let host = Host::default();
    run_until(
        console,
        &host,
        |_, _| {},
        |host| {
            decoded(&host.from_device.borrow())
                .iter()
                .any(|(_, words)| words == &[37])
        },
    );
    assert_eq!(EARLY_LOST.get(), 0);
}

#[test]
fn queued_notes_are_numbered_when_written_so_an_immediate_one_cannot_reorder_them() {
    let console = console();
    console.running.store(true, Ordering::Release);
    block_on(console.note_reliably(&rearm(1)));
    block_on(console.note_reliably(&rearm(2)));
    let first = console.notes.try_receive().expect("queued");
    let second = console.notes.try_receive().expect("queued");
    // Written in the order they were queued, each numbered at its write.
    let mut wire = Vec::new();
    let mut sink = Sink(&mut wire);
    block_on(console.write_note(&mut sink, &first));
    block_on(console.write_note(&mut sink, &second));
    let notes = decoded(&wire);
    assert_eq!(notes.len(), 2);
    // The sequence is shared by the process, so only its order is checked.
    assert!(notes[1].0 > notes[0].0);
    assert_eq!(notes[0].1, [1, 20, 0, 20_000, 0]);
    assert_eq!(notes[1].1, [2, 20, 0, 20_000, 0]);
}

#[test]
fn a_reliable_note_waits_for_room_instead_of_being_lost() {
    use core::{
        future::Future as _,
        task::{Context, Poll, Waker},
    };
    let console = console();
    console.running.store(true, Ordering::Release);
    for step in 0..4 {
        block_on(console.note_reliably(&rearm(step)));
    }
    let late = rearm(9);
    let mut waiting = core::pin::pin!(console.note_reliably(&late));
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(waiting.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(console.dropped_notes(), 0);
    let _ = console.notes.try_receive().expect("queued");
    assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(()));
}

/// An endpoint that keeps what is written.
struct Sink<'a>(&'a mut Vec<u8>);

impl embedded_io_async::ErrorType for Sink<'_> {
    type Error = core::convert::Infallible;
}

impl Write for Sink<'_> {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn a_control_reply_is_not_written_until_the_entire_frame_and_trailer_reach_the_endpoint() {
    use core::{
        future::Future as _,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};

    struct SplitTx<'a> {
        host: Host,
        gate: &'a Signal<NoopRawMutex, ()>,
        split: bool,
        open: bool,
    }
    impl embedded_io_async::ErrorType for SplitTx<'_> {
        type Error = core::convert::Infallible;
    }
    impl Write for SplitTx<'_> {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            if !self.split {
                self.split = true;
                let count = bytes.len() / 2;
                self.host
                    .from_device
                    .borrow_mut()
                    .extend_from_slice(&bytes[..count]);
                return Ok(count);
            }
            if !self.open {
                self.gate.wait().await;
                self.open = true;
            }
            self.host.from_device.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let console = console();
    console.start(BOOT);
    let sequence =
        block_on(console.publish_reliably(0, 9, &WatchdogArmed(WatchdogTestMode::Complete)));
    let message = console.messages.try_receive().expect("queued reply");
    let host = Host::default();
    let gate = Signal::new();
    let mut tx = SplitTx {
        host: host.clone(),
        gate: &gate,
        split: false,
        open: false,
    };
    let mut writing = core::pin::pin!(console.write_message(&mut tx, &message));
    let mut written = core::pin::pin!(console.written(sequence));
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(written.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(writing.as_mut().poll(&mut context), Poll::Pending);
    assert!(!host.from_device.borrow().is_empty());
    assert!(host.frames().is_empty());
    assert_eq!(written.as_mut().poll(&mut context), Poll::Pending);
    gate.signal(());
    assert_eq!(writing.as_mut().poll(&mut context), Poll::Ready(()));
    assert_eq!(written.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    assert_eq!(host.frames(), [(9, sequence, WatchdogArmed::KEY)]);
}

#[test]
fn failed_terminal_barrier_propagates_without_performing_the_terminal_action() {
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    let console = console();
    let mut cx = Context::from_waker(Waker::noop());
    let mut waiting: Vec<_> = (0..16).map(|_| Box::pin(console.written(10))).collect();
    for future in &mut waiting {
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    }
    let acted = core::cell::Cell::new(false);
    let mut terminal = core::pin::pin!(async {
        console.written(20).await?;
        acted.set(true);
        Ok(())
    });
    assert_eq!(
        terminal.as_mut().poll(&mut cx),
        Poll::Ready(Err(progress::ReceiverLimit {
            sequence: 20,
            next: Some(0),
            capacity: 16,
        }))
    );
    assert!(!acted.get());
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
        let console = console();
        let dropped = Rc::new(core::cell::Cell::new(false));
        let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED), 0);
        let failure = block_on(console.run(
            BrokenRead {
                length,
                dropped: dropped.clone(),
            },
            intake,
            &Chip,
            |_, _: crate::base::NoRequests| {},
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
fn counter_borrow_conflicts_return_typed_faults() {
    let console = console();
    console.received.lock(|received| {
        let _exclusive = received.borrow_mut();
        assert_eq!(
            console.link_health(),
            Err(ConsoleFault::CountersBorrowed { writing: false })
        );
    });
    let dropped = Rc::new(core::cell::Cell::new(false));
    let failure = console.received.lock(|received| {
        let _reader = received.borrow();
        let intake = Intake::new(BOOT, ImageKeySet::new(&SERVED), 0);
        block_on(console.run(
            BrokenRead {
                length: 0,
                dropped: dropped.clone(),
            },
            intake,
            &Chip,
            |_, _: crate::base::NoRequests| {},
        ))
    });
    assert_eq!(
        failure.cause,
        ConsoleFault::CountersBorrowed { writing: true }
    );
    assert!(!dropped.get());
    assert!(console.link_health().is_ok());
}

#[test]
fn note_length_overflow_refuses_the_whole_line() {
    let mut line = NoteLine::new();
    line.length = usize::MAX;
    line.encode(&Note::diag_lost(1));
    assert!(line.framed().is_empty());
    assert_eq!(line.bytes, [0; MAX_NOTE_LINE + 1]);
}
