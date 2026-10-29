use super::*;
use oer_ieee80211_instrumentation::*;
use oer_time::Clock;
use oer_time_virtual::VirtualClock;

fn event(at: u32, phase: Phase, level: u8) -> Activity {
    Activity::new(at, 0, 0, level, phase)
}

#[test]
fn idle_windows_clip_open_intervals_and_work_includes_preemption() {
    let clock = VirtualClock::<1>::new();
    let cpu = CpuCounter::new();
    let at = || clock.now().as_micros() as u32;
    cpu.observe(event(at(), Phase::WorkStart, 0));
    let start = cpu.sample(at()).unwrap();
    clock.advance(oer_time::Duration::from_micros(10)).unwrap();
    cpu.observe(event(at(), Phase::WorkEnd, 0));
    cpu.observe(event(at(), Phase::IdleStart, 0));
    clock.advance(oer_time::Duration::from_micros(20)).unwrap();
    let mid = cpu.sample(at()).unwrap();
    assert_eq!(start.until(mid).unwrap().busy_per_mille(), Some(333));
    clock.advance(oer_time::Duration::from_micros(10)).unwrap();
    cpu.observe(event(at(), Phase::IdleEnd, 0));
    cpu.observe(event(at(), Phase::WorkStart, 0));
    clock.advance(oer_time::Duration::from_micros(20)).unwrap();
    let end = cpu.sample(at()).unwrap();
    assert_eq!(
        start.until(end).unwrap(),
        CpuWindow {
            window_micros: 60,
            busy_micros: Some(30),
            work_inclusive_micros: 30
        }
    );
    assert_eq!(mid.until(end).unwrap().busy_per_mille(), Some(666));
    let irq = CpuCounter::new();
    irq.observe(event(10, Phase::WorkStart, 2));
    let irq_start = irq.sample(10).unwrap();
    irq.observe(event(30, Phase::WorkEnd, 2));
    let report = irq_start.until(irq.sample(60).unwrap()).unwrap();
    assert_eq!(report.work_inclusive_micros, 20);
    assert_eq!(
        report.busy_per_mille(),
        None,
        "only thread idle gives whole-hart load"
    );
}
#[test]
fn clock_and_totals_wrap_but_invalid_samples_are_explicit() {
    let cpu = CpuCounter::new();
    assert_eq!(cpu.sample(0), Err(SampleError::Unmeasured));
    cpu.observe(event(u32::MAX - 9, Phase::IdleStart, 0));
    let start = cpu.sample(u32::MAX - 9).unwrap();
    cpu.observe(event(10, Phase::IdleEnd, 0));
    let end = cpu.sample(10).unwrap();
    assert_eq!(start.until(end).unwrap().busy_per_mille(), Some(0));
    assert_eq!(end.until(end), Err(SampleError::Window));
    cpu.observe(event(20, Phase::IdleEnd, 0));
    assert_eq!(cpu.sample(20), Err(SampleError::Sequence));
    let invalid = CpuCounter::new();
    invalid.observe(event(0, Phase::IdleStart, 0));
    assert_eq!(invalid.sample(1 << 31), Err(SampleError::Window));
    invalid.sequence.store(1, Ordering::Relaxed);
    assert_eq!(invalid.sample(0), Err(SampleError::Concurrent));
}
#[test]
fn image_aggregates_weighted_events_and_keeps_missing_distinct_from_dropped() {
    let radio = RadioCounters::new();
    for _ in 0..2 {
        radio.observe(&RxContainment {
            ampdu: Evidence::HardwareYes,
            s_mpdu: Evidence::HardwareNo,
            bytes: Some(1200),
        });
        radio.observe(&RxReorder {
            tid: 0,
            cause: ReorderCause::Receive,
            held: 1,
            released: 0,
            missing: 0,
        });
    }
    radio.observe(&RxReorder {
        tid: 0,
        cause: ReorderCause::Timeout,
        held: 0,
        released: 2,
        missing: 1,
    });
    radio.observe(&RxReorder {
        tid: 0,
        cause: ReorderCause::Duplicate,
        held: 0,
        released: 0,
        missing: 0,
    });
    radio.observe(&RxReorder {
        tid: 0,
        cause: ReorderCause::Clear,
        held: 0,
        released: 1,
        missing: 0,
    });
    radio.observe(&RxBaStarted { tid: 0, window: 8 });
    radio.observe(&RxBaStopped {
        tid: 0,
        reason: BaStop::Peer,
    });
    radio.observe(&TxRateSelected { kbps: Some(65000) });
    let count = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
    assert_eq!(
        (
            count(&radio.ampdu_mpdus),
            count(&radio.ampdu_normalized_bytes)
        ),
        (2, 2400)
    );
    assert_eq!(
        (
            count(&radio.held),
            count(&radio.released),
            count(&radio.timeout_released)
        ),
        (2, 2, 2)
    );
    assert_eq!(
        (
            count(&radio.skipped),
            count(&radio.cleared),
            count(&radio.duplicates)
        ),
        (1, 1, 1)
    );
    assert_eq!((count(&radio.ba_started), count(&radio.ba_stopped)), (1, 1));
    assert_eq!(count(&radio.selected_kbps), 65000);
    assert!(!radio.incomplete.load(Ordering::Relaxed));
    radio
        .ampdu_normalized_bytes
        .store(u32::MAX, Ordering::Relaxed);
    radio.observe(&RxContainment {
        ampdu: Evidence::ProtocolYes,
        s_mpdu: Evidence::HardwareYes,
        bytes: Some(1),
    });
    assert!(radio.incomplete.load(Ordering::Relaxed));
    assert_eq!(count(&radio.ampdu_normalized_bytes), u32::MAX);
    radio.observe(&TxRateSelected { kbps: None });
    assert!(!radio.rate_known.load(Ordering::Relaxed));
}

#[test]
fn idle_fraction_is_clipped_at_both_window_boundaries() {
    let cpu = CpuCounter::new();
    let clock = VirtualClock::<1>::new();
    let at = || clock.now().as_micros() as u32;
    cpu.observe(event(at(), Phase::IdleStart, 0));
    clock.advance(oer_time::Duration::from_micros(10)).unwrap();
    let start = cpu.sample(at()).unwrap();
    clock.advance(oer_time::Duration::from_micros(30)).unwrap();
    let during = cpu.sample(at()).unwrap();
    assert_eq!(start.until(during).unwrap().busy_per_mille(), Some(0));
    cpu.observe(event(at(), Phase::IdleEnd, 0));
    cpu.observe(event(at(), Phase::WorkStart, 0));
    clock.advance(oer_time::Duration::from_micros(10)).unwrap();
    cpu.observe(event(at(), Phase::WorkEnd, 0));
    cpu.observe(event(at(), Phase::IdleStart, 0));
    clock.advance(oer_time::Duration::from_micros(10)).unwrap();
    let end = cpu.sample(at()).unwrap();
    let window = start.until(end).unwrap();
    assert_eq!(window.window_micros, 50);
    assert_eq!(window.busy_micros, Some(10));
    assert_eq!(window.busy_per_mille(), Some(200));
}
