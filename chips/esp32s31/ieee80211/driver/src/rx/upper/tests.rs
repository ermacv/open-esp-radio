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

use oer_esp32s31_hal::ieee80211::dma::descriptor::rx_armed_word;
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
fn ends(
    counters: &'static WifiRxCounters,
) -> RxFrameEnds<'static, SLOTS, BUFFER, oer_espressif_interrupt_table::signal::host::Host> {
    let (publisher, _consumer) = leak(IrqStorage::new(
        oer_espressif_interrupt_table::signal::host::signal().0,
    ))
    .split();
    let publisher: &'static IrqPublisher<
        'static,
        oer_espressif_interrupt_table::signal::host::Host,
    > = leak(publisher);
    RxFrameQueue::init_in_place(leak(core::mem::MaybeUninit::uninit()))
        .split(counters, RxReturnNotifier::new(publisher))
}

/// Poll the receiver once.
fn poll_next<'a>(
    receiver: &mut FrameReceiver<
        'a,
        SLOTS,
        BUFFER,
        oer_espressif_interrupt_table::signal::host::Host,
    >,
) -> Option<HeldRxFrame<'a, 'a, SLOTS, BUFFER, oer_espressif_interrupt_table::signal::host::Host>> {
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
    receiver:
        FrameReceiver<'static, SLOTS, BUFFER, oer_espressif_interrupt_table::signal::host::Host>,
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
    assert!(rx_armed_word(oversized.iter().sum::<u32>()).is_none());
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

mod normalized_handoff {
    use oer_esp32s31_ieee80211_mac::portable::rx_meta;
    use oer_ieee80211_port::{Channel, ChannelWidth, RxEvidence};
    use oer_time::Instant;

    use super::*;

    const STAND: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const OTHER: [u8; 6] = [2, 0, 0, 0, 0, 2];
    /// 100 TU in µs.
    const BI: u64 = 102_400;
    const CHANNEL: u8 = 6;

    /// A beacon (subtype 8) or probe response (subtype 5) from `bss`.
    fn management(subtype: u8, bss: [u8; 6], tsf: u64, ssid: &[u8], ds: Option<u8>) -> Vec<u8> {
        let mut frame = std::vec![0_u8; 36];
        frame[0] = subtype << 4;
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&bss);
        frame[16..22].copy_from_slice(&bss);
        frame[24..32].copy_from_slice(&tsf.to_le_bytes());
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[34] = 0x01;
        frame.extend_from_slice(&[0, ssid.len() as u8]);
        frame.extend_from_slice(ssid);
        if let Some(channel) = ds {
            frame.extend_from_slice(&[3, 1, channel]);
        }
        frame
    }

    /// One unit the ring completes: its MPDU, prefix RSSI, the time the
    /// runner hands it off and its handoff lag behind the prefix stamp.
    struct Unit {
        mpdu: Vec<u8>,
        rssi: i8,
        handoff: u64,
        lag: u32,
        rx_state: u8,
    }

    fn unit(mpdu: Vec<u8>, rssi: i8, handoff: u64, lag: u32) -> Unit {
        Unit {
            mpdu,
            rssi,
            handoff,
            lag,
            rx_state: 0,
        }
    }

    #[test]
    fn normalized_frames_preserve_bytes_signal_and_timing_through_service() {
        let mut fixture = fixture();
        let mut normalized = 0;
        let channel = Channel::ghz2_4(CHANNEL, ChannelWidth::Mhz20).unwrap();
        // One second past the clock's start.
        let base = oer_time_virtual::START.as_micros() + 1_000_000;
        let stand = |tsf| management(8, STAND, tsf, b"stand", Some(CHANNEL));
        let mut data = std::vec![0_u8; 24];
        data[0] = 0x08;
        data[10..16].copy_from_slice(&OTHER);
        let units = [
            unit(stand(0), -40, base, 30),
            unit(
                management(8, OTHER, 5_000, b"", None),
                -70,
                base + 10_000,
                30,
            ),
            unit(stand(BI), -44, base + BI, 45),
            unit(
                management(5, OTHER, 6_000, b"hidden", None),
                -68,
                base + BI + 2_000,
                30,
            ),
            // The same beacon again: a repeated TSF.
            unit(stand(BI), -42, base + BI + 4_000, 30),
            // Two beacons missed: a late gap of the stand BSSID.
            unit(stand(4 * BI), -46, base + 4 * BI, 30),
            unit(data, -50, base + 4 * BI + 1_000, 30),
            // A malformed prefix never reaches the survey.
            Unit {
                rx_state: 0xf5,
                ..unit(stand(5 * BI), -40, base + 5 * BI, 30)
            },
        ];

        let mut stamps = Vec::new();
        for unit in &units {
            let handoff = Instant::from_micros(unit.handoff);
            let stamp = unit.handoff as u32 - unit.lag;
            stamps.push(stamp);
            fixture.clock.advance_to(handoff);
            let prefix = S31Prefix {
                rssi: unit.rssi,
                stamp,
                rx_state: unit.rx_state,
                ..S31Prefix::new()
            };
            fixture.ring.complete_with(&prefix.unit(&unit.mpdu));
            assert_eq!(fixture.settle(), DatapathRxProgress::Drained);

            let held = poll_next(&mut fixture.receiver).expect("the staged unit");
            let times = held.times().expect("the sink marked the handoff");
            // The handoff survey.take expects is the clock time QueueSink marked.
            assert_eq!(times.handoff, handoff);
            if let Ok(frame) = fixture.receiver.normalize(&held) {
                assert_eq!(frame.stamp, Some(stamp));
                let meta = rx_meta(frame.metadata, channel);
                assert_eq!(frame.mpdu, unit.mpdu.as_slice());
                assert_eq!(meta.channel, channel);
                assert_eq!(meta.rssi_dbm, RxEvidence::HardwareObserved(unit.rssi));
                normalized += 1;
            }
            drop(held);
        }

        // Every lease came back.
        assert_eq!(fixture.ring.pool.claimed_slots(), 0);
        assert_eq!(fixture.free_capacity(), SLOTS);
        assert_eq!(fixture.counters.report().units, units.len() as u32);
        assert_eq!(fixture.counters.report().prefix.mic_failure, 1);

        assert_eq!(normalized, 7);
    }
}

#[test]
fn a_finite_dwell_drains_queued_leases_without_waiting_or_discarding_them() {
    let mut fixture = fixture();
    assert!(fixture.receiver.try_next().is_none());
    let beacon = S31Prefix::new().unit(&management_header(8));
    fixture.ring.complete_with(&beacon);
    fixture.settle();
    let held = fixture.receiver.try_next().expect("the staged frame");
    assert!(fixture.receiver.try_next().is_none());
    assert_eq!(fixture.ring.pool.available_slots(), SLOTS - 1);
    drop(held);
    assert_eq!(fixture.ring.pool.available_slots(), SLOTS);
}
