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

const BOOT: u64 = 77;

type TestConsole = Console<64, 4, 8>;

fn console() -> &'static TestConsole {
    Box::leak(Box::new(TestConsole::new(|line| {
        IMMEDIATE.with(|lines| lines.borrow_mut().push(line.to_bytes().to_vec()));
    })))
}

std::thread_local! {
    static IMMEDIATE: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
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

struct Rx(Host);
struct Tx(Host);

impl embedded_io_async::ErrorType for Rx {
    type Error = core::convert::Infallible;
}
impl embedded_io_async::ErrorType for Tx {
    type Error = core::convert::Infallible;
}

impl Read for Rx {
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

impl Write for Tx {
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
        Rx(host.clone()),
        Tx(host.clone()),
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
        Either::First(never) => never,
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
    assert_eq!(console.link_health().tx_dropped, 1);
}

#[test]
fn text_before_the_console_runs_is_written_immediately() {
    let console = console();
    IMMEDIATE.with(|lines| lines.borrow_mut().clear());
    console.line(format_args!("early {}", 1));
    IMMEDIATE.with(|lines| assert_eq!(*lines.borrow(), [b"early 1".to_vec()]));
}

#[test]
fn a_cut_line_is_counted() {
    let console = console();
    console.line_immediately(format_args!("{}", "x".repeat(100)));
    assert_eq!(console.truncated_lines(), 1);
}
