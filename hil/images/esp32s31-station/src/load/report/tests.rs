use super::*;
use oer_espressif::observe::parts::{LIBRARY_PARTS, PartSample};
use oer_trace::profile::{PcSample, ReturnSample};

fn executor(executor: u8, hart: u8, work_us: Option<u64>, idle_us: Option<u64>) -> ExecutorSample {
    ExecutorSample {
        executor,
        hart,
        level: executor,
        work_us,
        idle_us,
    }
}

#[test]
fn the_cpu_report_sums_each_harts_idle_and_lists_each_working_executor() {
    let samples = [
        executor(0, 0, Some(100), Some(700)),
        executor(1, 0, Some(50), None),
        executor(2, 1, Some(20), Some(900)),
        executor(3, 1, None, Some(5)),
    ];
    let load = load(1_000, &samples);
    assert_eq!(load.now_us, 1_000);
    assert_eq!(
        load.harts
            .iter()
            .map(|hart| (hart.hart, hart.idle_us))
            .collect::<Vec<_>>(),
        [(0, 700), (1, 905)]
    );
    assert_eq!(
        load.executors
            .iter()
            .map(|executor| (executor.executor, executor.hart, executor.work_us))
            .collect::<Vec<_>>(),
        [(0, 0, 100), (1, 0, 50), (2, 1, 20)],
        "an executor that saw no work reports none"
    );
}

fn reading(name: Option<&'static str>, polls: u32, us: u32, longest: Option<u32>) -> PartReading {
    PartReading {
        name,
        sample: PartSample {
            polls,
            poll_us: us.wrapping_add(3),
            ended_us: us,
            longest_us: longest,
        },
    }
}

#[test]
fn a_window_holds_the_named_parts_differences_and_the_ends_longest_poll() {
    let mut start = [PartReading::default(); PARTS];
    let mut end = [PartReading::default(); PARTS];
    start[0] = reading(Some("application"), u32::MAX, u32::MAX - 1, Some(9));
    end[0] = reading(Some("application"), 4, 8, Some(5));
    // The image named its first part between the window's ends: it has no
    // start and reports nothing.
    end[LIBRARY_PARTS] = reading(Some("load"), 7, 70, Some(10));
    start[LIBRARY_PARTS + 1] = reading(Some("relay"), 1, 10, None);
    end[LIBRARY_PARTS + 1] = reading(Some("relay"), 3, 40, Some(20));
    let windows = parts(&start, &end);
    let rows: Vec<_> = windows
        .iter()
        .map(|part| {
            (
                part.name.as_str(),
                part.polls,
                part.poll_us,
                part.ended_poll_us,
                part.longest_poll_us,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("application", 5, 10, 10, Some(5)),
            ("relay", 2, 30, 30, Some(20)),
        ]
    );
}

#[test]
fn pages_of_parts_start_at_their_number_and_hold_at_most_one_answer() {
    let mut start = [PartReading::default(); PARTS];
    for (number, reading) in start.iter_mut().enumerate() {
        *reading = PartReading {
            name: Some(["a", "b", "c"][number % 3]),
            sample: PartSample::default(),
        };
    }
    let windows = parts(&start, &start);
    assert_eq!(windows.len(), PARTS);
    assert_eq!(page(&windows, 0).parts.len(), MAX_PARTS);
    assert_eq!(
        page(&windows, MAX_PARTS as u8).parts.len(),
        PARTS - MAX_PARTS
    );
    assert!(page(&windows, PARTS as u8).parts.is_empty());
}

#[test]
fn the_tracking_window_counts_differences_with_the_ends_extremes() {
    let start = TrackingSample {
        ticks: u32::MAX,
        not_due: 2,
        tick_us: 100,
        temperature_last: Some(1),
        longest_tick_us: Some(90),
        ..TrackingSample::default()
    };
    let end = TrackingSample {
        ticks: 4,
        not_due: 3,
        calibration_common: 2,
        tick_us: 400,
        temperature_last: Some(-3),
        temperature_min: Some(-5),
        temperature_max: Some(2),
        longest_tick_us: Some(70),
        ..TrackingSample::default()
    };
    let window = tracking(&start, &end);
    assert!(window.available);
    assert_eq!(
        (
            window.ticks,
            window.not_due,
            window.calibration_common,
            window.tick_us
        ),
        (5, 1, 2, 300)
    );
    assert_eq!(
        (
            window.temperature_last,
            window.temperature_min,
            window.temperature_max,
            window.longest_tick_us
        ),
        (Some(-3), Some(-5), Some(2), Some(70))
    );
}

#[test]
fn the_aggregates_histogram_wraps_at_two_to_the_sixteen() {
    let ampdu = link::TxAmpdu {
        aggregates: 70_000,
        by_size: [0x1_0002, 3, 4, 5, 6, 0xffff],
        acknowledged: 9,
        ..link::TxAmpdu::default()
    };
    let wire = tx_ampdu(&ampdu);
    assert_eq!(wire.aggregates, 70_000);
    assert_eq!(
        [
            wire.subframes_1,
            wire.subframes_2,
            wire.subframes_3_4,
            wire.subframes_5_8,
            wire.subframes_9_16,
            wire.subframes_17_32
        ],
        [2, 3, 4, 5, 6, 0xffff]
    );
    assert_eq!(wire.acknowledged, 9);
}

#[test]
fn the_link_keeps_every_rate_and_end() {
    let mut counted = link::Link::default();
    counted.rx.mpdus.ht40_short_gi[7] = 11;
    counted.rx.in_ampdu = 5;
    counted.tx.mpdus.non_ht = 2;
    counted.tx.ack_timeouts = 1;
    let wire = link(&counted);
    assert_eq!(wire.rx.mpdus.ht40_short_gi[7], 11);
    assert_eq!(wire.rx.in_ampdu, 5);
    assert_eq!(wire.tx.mpdus.non_ht, 2);
    assert_eq!(wire.tx.ack_timeouts, 1);
}

#[test]
fn the_counters_window_takes_differences_and_keeps_an_overflow() {
    let mut start = Snapshot {
        cycles: 100,
        instructions: 50,
        stack_received: 10,
        ..Snapshot::default()
    };
    start.bridge.received = u32::MAX;
    start.cache.ibus0.hits = 5;
    start.cache.dbus1.overflowed = true;
    let mut end = Snapshot {
        cycles: 400,
        instructions: 80,
        stack_received: 4,
        ..Snapshot::default()
    };
    end.bridge.received = 2;
    end.bridge.refused_queue = 1;
    end.cache.ibus0.hits = 9;
    end.cache_geometry.l2_bypassed = true;
    let window = counters(&start, &end);
    assert_eq!((window.cycles, window.instructions), (300, 30));
    assert_eq!(
        (window.bridge_received, window.bridge_refused_queue),
        (3, 1)
    );
    assert_eq!(
        window.stack_received, 0,
        "a stack count that went back saturates"
    );
    assert_eq!(window.cache.ibus0.hits, 4);
    assert!(window.cache.dbus1.overflowed);
    assert!(window.cache_geometry.l2_bypassed);
}

#[test]
fn pages_of_samples_cover_every_kept_sample_once_and_stop_at_the_end() {
    const CAPACITY: usize = 256;
    let mut buffer = Box::new(Thinning::<CAPACITY>::new());
    buffer.restart(1000);
    let ticks = CAPACITY as u32 * 3;
    for tick in 0..ticks {
        buffer.record(PcSample::new(tick, 0x4000_0000 + tick, 1).unwrap());
        if tick.is_multiple_of(2) {
            buffer.record_return(ReturnSample::new(tick, 0x4200_0000 + tick).unwrap());
        }
    }
    buffer.normalize();
    let mut seen = Vec::new();
    let mut from = 0;
    loop {
        let page = samples(&buffer, from, ticks, 7, false);
        assert_eq!(
            (page.ticks, page.stride, page.kept as usize, page.stray),
            (ticks, buffer.stride(), buffer.len(), 7)
        );
        if page.samples.is_empty() {
            break;
        }
        assert!(page.samples.len() <= MAX_PAGE_SAMPLES);
        for sample in &page.samples {
            let ra = sample
                .tick
                .is_multiple_of(2)
                .then_some(0x4200_0000 + sample.tick);
            assert_eq!(sample.ra, ra, "tick {}", sample.tick);
        }
        seen.extend(page.samples.iter().map(|sample| sample.tick));
        from += page.samples.len() as u16;
    }
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..ticks)
            .step_by(buffer.stride() as usize)
            .collect::<Vec<_>>()
    );
    assert!(
        samples(&buffer, u16::MAX, ticks, 0, false)
            .samples
            .is_empty()
    );
}

#[test]
fn the_link_window_takes_the_rates_differences() {
    let mut start = Snapshot::default();
    start.link.rx.mpdus.ht20_long_gi[3] = 4;
    start.link.tx.delivered = 1;
    let mut end = start;
    end.link.rx.mpdus.ht20_long_gi[3] = 10;
    end.link.tx.delivered = 5;
    let window = link_window(&start, &end);
    assert_eq!(window.rx.mpdus.ht20_long_gi[3], 6);
    assert_eq!(window.tx.delivered, 4);
}
