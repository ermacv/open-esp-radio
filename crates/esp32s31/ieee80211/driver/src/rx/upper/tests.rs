use core::{
    future::Future,
    pin::pin,
    task::{Context, Waker},
};
use std::{
    boxed::Box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
    vec::Vec,
};

use oer_esp32s31_ieee80211_dma::descriptor::SIZE_MASK;
use oer_esp32s31_ieee80211_mac::irq::consumer::{IrqPublisher, IrqStorage};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use super::*;
use crate::{
    datapath::DatapathRxProgress,
    rx::{
        sink::CountingAdmission,
        test_ring::{BUFFER, Ring, S31Prefix, SLOTS, management_header},
    },
};

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

/// The two ends of a leaked queue whose returns post to a leaked fact
/// storage.
fn ends(counters: &'static WifiRxCounters) -> RxFrameEnds<'static, SLOTS, BUFFER> {
    let (publisher, _consumer) = leak(IrqStorage::new()).split();
    let publisher: &'static IrqPublisher<'static> = leak(publisher);
    leak(RxFrameQueue::new()).split(counters, RxReturnNotifier::new(publisher))
}

/// Poll the receiver once.
fn poll_next<'a>(
    receiver: &mut FrameReceiver<'a, SLOTS, BUFFER>,
) -> Option<HeldRxFrame<'a, 'a, SLOTS, BUFFER>> {
    let mut next = pin!(receiver.next());
    match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(frame) => Some(frame),
        Poll::Pending => None,
    }
}

struct Fixture {
    counters: &'static WifiRxCounters,
    clock: &'static VirtualClock,
    sink: QueueSink<'static, VirtualClock, SLOTS, BUFFER>,
    receiver: FrameReceiver<'static, SLOTS, BUFFER>,
    admission: CountingAdmission<'static>,
    ring: Ring,
}

fn fixture() -> Fixture {
    let counters: &'static WifiRxCounters = leak(WifiRxCounters::new());
    let clock: &'static VirtualClock = leak(VirtualClock::new());
    let RxFrameEnds { sender, receiver } = ends(counters);
    Fixture {
        counters,
        clock,
        sink: QueueSink::new(sender, clock),
        receiver,
        admission: CountingAdmission::new(counters),
        ring: Ring::new(),
    }
}

impl Fixture {
    fn settle(&mut self) -> DatapathRxProgress {
        self.ring.settle(&self.sink, &self.admission)
    }

    fn free_capacity(&self) -> usize {
        Publisher::free_capacity(&self.sink)
    }
}

#[test]
fn queue_sink_free_capacity_is_exact_and_try_send_never_refuses() {
    let mut fixture = fixture();
    let stamped = fixture.clock.advance(Duration::from_micros(1_234)).unwrap();
    assert_eq!(fixture.free_capacity(), SLOTS);
    let beacon = S31Prefix::new().unit(&management_header(8));
    for _ in 0..SLOTS + 2 {
        fixture.ring.complete_with(&beacon);
    }

    // One frame per pool credit, the reserve included for management: every
    // send after a credit succeeded (a refusal would fault the service).
    assert_eq!(fixture.settle(), DatapathRxProgress::StageCapacityBlocked);
    assert_eq!(fixture.counters.report().units, SLOTS as u32);
    assert_eq!(fixture.free_capacity(), 0);
    assert_eq!(fixture.ring.pool.available_slots(), 0);

    // P1 takes one frame: exactly one cell frees, while the held frame
    // keeps its pool slot until it is dropped.
    let held = poll_next(&mut fixture.receiver).expect("a queued frame");
    assert_eq!(fixture.free_capacity(), 1);
    assert_eq!(fixture.ring.pool.available_slots(), 0);
    assert_eq!(
        held.times(),
        Some(RxTimes {
            handoff: stamped,
            stamp: Some(S31Prefix::new().stamp),
        })
    );
    assert_eq!(held.length(), beacon.len());
    drop(held);
    assert_eq!(fixture.ring.pool.available_slots(), 1);
    assert_eq!(fixture.settle(), DatapathRxProgress::StageCapacityBlocked);
    assert_eq!(fixture.counters.report().units, SLOTS as u32 + 1);
    assert_eq!(fixture.free_capacity(), 0);

    // P1 drains everything; the last unit follows.
    while let Some(held) = poll_next(&mut fixture.receiver) {
        drop(held);
    }
    assert_eq!(fixture.free_capacity(), SLOTS);
    assert_eq!(fixture.settle(), DatapathRxProgress::Drained);
    assert_eq!(fixture.counters.report().units, SLOTS as u32 + 2);
    assert_eq!(fixture.free_capacity(), SLOTS - 1);
}

/// Counts its wakes.
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_send_wakes_the_waiting_receiver() {
    let mut fixture = fixture();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut context = Context::from_waker(&waker);
    let mut next = pin!(fixture.receiver.next());
    assert!(next.as_mut().poll(&mut context).is_pending());

    fixture
        .ring
        .complete_with(&S31Prefix::new().unit(&management_header(5)));
    assert_eq!(
        fixture.ring.settle(&fixture.sink, &fixture.admission),
        DatapathRxProgress::Drained
    );
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(next.as_mut().poll(&mut context).is_ready());
}

#[test]
fn chained_and_oversized_units_never_reach_p1() {
    let mut fixture = fixture();
    fixture.ring.complete(&[BUFFER as u32, 100]);
    let mut oversized = [BUFFER as u32; 10];
    oversized[9] = 1_500;
    assert!(oversized.iter().sum::<u32>() > SIZE_MASK);
    fixture.ring.complete(&oversized);
    let beacon = S31Prefix::new().unit(&management_header(8));
    fixture.ring.complete_with(&beacon);
    assert_eq!(fixture.settle(), DatapathRxProgress::Drained);

    let report = fixture.counters.report();
    assert_eq!(
        (report.chained_drops, report.oversized_drops, report.units),
        (1, 1, 1)
    );
    let held = poll_next(&mut fixture.receiver).expect("the single unit");
    assert_eq!(held.segment().buffer, &beacon[..]);
    assert!(poll_next(&mut fixture.receiver).is_none());
}

#[test]
fn malformed_prefix_refusals_are_counted_by_reason() {
    let mut fixture = fixture();
    let mpdu = management_header(8);
    let units = [
        S31Prefix {
            rx_state: 0xf5,
            ..S31Prefix::new()
        },
        S31Prefix {
            rx_state: 0xc6,
            ..S31Prefix::new()
        },
        // `sig_len` shorter than a frame control and the FCS.
        S31Prefix {
            signal_length: Some(5),
            ..S31Prefix::new()
        },
        // Admitted under W1, refused by either strict check.
        S31Prefix {
            rxend_state: 1,
            dump_length: Some(0),
            ..S31Prefix::new()
        },
    ];
    for prefix in units {
        fixture.ring.complete_with(&prefix.unit(&mpdu));
    }
    assert_eq!(fixture.settle(), DatapathRxProgress::Drained);
    assert_eq!(fixture.counters.report().units, 4);

    let mut results = Vec::new();
    while let Some(held) = poll_next(&mut fixture.receiver) {
        results.push(
            fixture
                .receiver
                .normalize(&held)
                .map(|frame| frame.mpdu.to_vec()),
        );
    }
    assert_eq!(
        results,
        [
            Err(RxError::MicFailure),
            Err(RxError::Quarantined),
            Err(RxError::Bounds),
            Ok(mpdu.to_vec()),
        ]
    );
    let prefix = fixture.counters.report().prefix;
    assert_eq!(
        (
            prefix.refused(RxError::MicFailure),
            prefix.refused(RxError::Quarantined),
            prefix.refused(RxError::Bounds),
            prefix.refused(RxError::RxFailure),
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        (prefix.strict_rxend_refusable, prefix.strict_dump_refusable),
        (1, 1)
    );
}
