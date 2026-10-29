#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use super::*;
use collector::Collector;
use oer_trace::{Channel, Domain, Event, Kind, Observer};

#[derive(Clone, Copy)]
pub(super) struct Step;
impl Event for Step {
    const KIND: Kind = Kind::new(Domain::Platform, 200);
    const CHANNEL: Channel = match Channel::new(Domain::Platform, 0) {
        Ok(channel) => channel,
        Err(_) => panic!("constant channel"),
    };
    fn encode(&self) -> [u32; 2] {
        [7, 9]
    }
    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [7, 9]).then_some(Self)
    }
}
pub(super) const ID: Identity = Identity {
    capture: 1,
    epoch: 2,
};
pub(super) fn request(first: u32, count: u8) -> PageRequest {
    PageRequest {
        identity: ID,
        first,
        count,
    }
}

#[derive(Clone, Copy)]
struct Other(u8);
impl Event for Other {
    const KIND: Kind = Kind::new(Domain::Platform, 201);
    const CHANNEL: Channel = Step::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        panic!("atomic counters must not encode event payloads")
    }
    fn decode(_: [u32; 2]) -> Option<Self> {
        None
    }
}
#[test]
fn kinds_count_independently_include_supported_zeros_and_validate_reads() {
    let collector =
        Collector::new([Step::KIND, Other::KIND, Kind::new(Domain::Platform, 202)]).unwrap();
    collector.event(Step);
    collector.event(Step);
    let other = Other(9);
    assert_eq!(other.0, 9);
    collector.event(other);
    let reader = collector.reader(ID);
    let page = reader.counters(request(0, 3)).unwrap();
    assert_eq!(
        page.counters
            .iter()
            .map(|counter| counter.value)
            .collect::<std::vec::Vec<_>>(),
        [2, 1, 0]
    );
    assert_eq!(reader.describe().unwrap().records, 0);
    assert_eq!(reader.counters(request(1, 3)), Err(Error::InvalidRange));
    assert_eq!(
        reader.counters(PageRequest {
            identity: Identity { epoch: 3, ..ID },
            ..request(0, 1)
        }),
        Err(Error::IdentityMismatch)
    );
    assert!(matches!(
        Request::Records(ReadRecords(request(0, 1))).serve(&reader),
        Answer::Refused(Refused(Error::InvalidRange))
    ));
    assert!(matches!(
        Collector::new([Step::KIND, Step::KIND]),
        Err(Error::InvalidState)
    ));
}

#[test]
fn service_keys_only_appear_when_mounted_and_maximum_pages_fit_frames() {
    use crate::base::Service;
    use oer_hil_protocol::{Envelope, FrameEncoder};
    crate::image_keys! { MOUNTED = [Request] }
    crate::image_keys! { PLAIN = [] }
    for key in Request::KEYS {
        assert!(MOUNTED.contains(*key));
        assert!(!PLAIN.contains(*key));
    }
    let mut records = heapless::Vec::new();
    for _ in 0..RECORDS_PER_PAGE {
        records
            .push(EncodedRecord::from_slice(&[u8::MAX; RECORD_BYTES]).unwrap())
            .unwrap();
    }
    let mut encoder = FrameEncoder::new();
    assert!(
        encoder
            .encode(&Envelope::new(
                u64::MAX,
                u32::MAX,
                u64::MAX,
                u32::MAX,
                Records {
                    identity: Identity {
                        capture: u32::MAX,
                        epoch: u32::MAX
                    },
                    first: u32::MAX,
                    total: u32::MAX,
                    records
                }
            ))
            .is_ok()
    );
    let counters = (0..COUNTERS_PER_PAGE)
        .map(|_| Counter {
            kind: u16::MAX,
            value: u32::MAX,
            saturated: true,
        })
        .collect();
    assert!(
        encoder
            .encode(&Envelope::new(
                u64::MAX,
                u32::MAX,
                u64::MAX,
                u32::MAX,
                Counters {
                    identity: ID,
                    first: u32::MAX,
                    total: u16::MAX,
                    counters
                }
            ))
            .is_ok()
    );
}

#[test]
fn fake_console_serves_the_same_capture_and_a_typed_refusal() {
    use crate::{
        base::{Intake, Platform},
        console::{Console, Immediate},
    };
    use embassy_futures::{block_on, select::select};
    use oer_hil_protocol::{
        Envelope, FrameDecoder, FrameEncoder, Message, WireKind,
        base::{BootEvidence, PostMortemCheckpoints, ResetReason},
    };
    use std::{cell::RefCell, collections::VecDeque, rc::Rc, vec::Vec};
    struct Early;
    impl Immediate for Early {
        fn note(_: &oer_diagnostics::Note<'_>) -> bool {
            false
        }
        fn take_lost() -> u32 {
            0
        }
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
    #[derive(Clone, Default)]
    struct Host {
        input: Rc<RefCell<VecDeque<Vec<u8>>>>,
        output: Rc<RefCell<Vec<u8>>>,
    }
    impl Host {
        fn send<M: Message>(&self, request: u32, body: M) {
            let mut encoder = FrameEncoder::new();
            self.input.borrow_mut().push_back(
                encoder
                    .encode(&Envelope::new(77, 0, 0, request, body))
                    .unwrap()
                    .to_vec(),
            );
        }
    }
    struct Endpoint(Host);
    impl embedded_io_async::ErrorType for Endpoint {
        type Error = core::convert::Infallible;
    }
    impl embedded_io_async::Read for Endpoint {
        async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
            loop {
                if let Some(input) = self.0.input.borrow_mut().pop_front() {
                    bytes[..input.len()].copy_from_slice(&input);
                    return Ok(input.len());
                }
                embassy_futures::yield_now().await;
            }
        }
    }
    impl embedded_io_async::Write for Endpoint {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            self.0.output.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    let collector = Collector::new([Step::KIND]).unwrap();
    collector.event(Step);
    let reader = collector.reader(ID);
    let console = Console::<1, 8, Early>::new();
    console.start(77);
    crate::image_keys! { KEYS = [Request] }
    let host = Host::default();
    host.send(1, Describe);
    host.send(2, ReadRecords(request(0, 1)));
    host.send(3, ReadCounters(request(0, 1)));
    host.send(4, ReadRecords(request(1, 1)));
    let run = console.run(
        Endpoint(host.clone()),
        Intake::new(77, KEYS, 0),
        &Chip,
        |identity, request: Request| block_on(request.serve(&reader).publish(&console, identity)),
    );
    let watch = async {
        for _ in 0..1000 {
            let mut decoder = FrameDecoder::new();
            let mut received = Vec::new();
            decoder.feed(WireKind::Event, &host.output.borrow(), |frame| {
                let frame = frame.unwrap();
                if frame.request_id != 0 {
                    received.push((frame.request_id, frame.key));
                }
                if let Some(refused) = frame.decode::<Refused>() {
                    assert_eq!(refused.unwrap().body, Refused(Error::InvalidRange));
                }
            });
            if received.len() == 4 {
                assert_eq!(
                    received,
                    [
                        (1, Description::KEY),
                        (2, Refused::KEY),
                        (3, Counters::KEY),
                        (4, Refused::KEY)
                    ]
                );
                return;
            }
            embassy_futures::yield_now().await;
        }
        panic!("fake console did not answer capture requests");
    };
    assert!(matches!(
        block_on(select(run, watch)),
        embassy_futures::select::Either::Second(())
    ));
}
