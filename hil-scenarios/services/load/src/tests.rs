extern crate std;

use super::header::{Header, LENGTH};
use super::*;

#[test]
fn a_header_reads_back_what_was_written_and_refuses_other_payloads() {
    let header = Header {
        run: 0x0102_0304,
        sequence: u64::MAX - 1,
        sent_us: 123_456_789,
        echo_request: true,
        echo_reply: false,
    };
    let mut payload = [0xa5; 1400];
    header.write(&mut payload).unwrap();
    assert_eq!(Header::read(&payload), Some(header));
    assert_eq!(&payload[..4], b"OERL");
    assert_eq!(payload[LENGTH], 0xa5, "the filler stays");
    assert!(Header::read(&payload[..LENGTH - 1]).is_none());
    assert!(header.write(&mut [0; LENGTH - 1]).is_none());
    let mut foreign = payload;
    foreign[0] = b'X';
    assert!(Header::read(&foreign).is_none());
    let mut unknown_flag = payload;
    unknown_flag[4] = 4;
    assert!(Header::read(&unknown_flag).is_none());
}

#[test]
fn the_largest_reports_fit_one_response() {
    let load = Load {
        now_us: u64::MAX,
        harts: (0..MAX_HARTS as u8)
            .map(|hart| HartLoad {
                hart,
                idle_us: u64::MAX,
            })
            .collect(),
        executors: (0..MAX_EXECUTORS as u8)
            .map(|executor| ExecutorLoad {
                executor,
                hart: 1,
                level: 3,
                work_us: u64::MAX,
            })
            .collect(),
    };
    let mut buffer = [0; 1024];
    assert!(postcard::to_slice(&load, &mut buffer).unwrap().len() <= 256);
    let report = Collected::Report(Report {
        window_start_us: u64::MAX,
        window_end_us: u64::MAX,
        cpu_start: load.clone(),
        cpu_end: load.clone(),
        received: Received {
            datagrams: u64::MAX,
            highest: Some(u64::MAX),
            first_us: Some(u64::MAX),
            last_us: Some(u64::MAX),
            ..Received::default()
        },
        sent: Sent {
            datagrams: u64::MAX,
            first_us: Some(u64::MAX),
            last_us: Some(u64::MAX),
            ..Sent::default()
        },
    });
    assert!(postcard::to_slice(&report, &mut buffer).unwrap().len() <= 512);
    let part = PartWindow {
        name: heapless::String::try_from("x".repeat(MAX_PART_NAME).as_str()).unwrap(),
        polls: u32::MAX,
        poll_us: u32::MAX,
        ended_poll_us: u32::MAX,
        longest_poll_us: Some(u32::MAX),
    };
    let parts = PartsCollected::Report(Parts {
        parts: core::iter::repeat_n(part, MAX_PARTS).collect(),
    });
    assert!(postcard::to_slice(&parts, &mut buffer).unwrap().len() <= 512);
    let counters = CountersCollected::Report(Counters {
        cycles: u64::MAX,
        instructions: u64::MAX,
        bridge_received: u32::MAX,
        bridge_refused_pool: u32::MAX,
        bridge_refused_queue: u32::MAX,
        bridge_refused_other: u32::MAX,
        bridge_copied: u32::MAX,
        stack_received: u32::MAX,
        cache: full_cache(),
        cache_geometry: full_geometry(),
    });
    assert!(postcard::to_slice(&counters, &mut buffer).unwrap().len() <= 512);
    let samples = SamplesCollected::Page(Samples {
        period_us: u32::MAX,
        ticks: u32::MAX,
        stride: u32::MAX,
        kept: u16::MAX,
        stray: u32::MAX,
        cut: true,
        samples: core::iter::repeat_n(
            Sample {
                tick: u32::MAX,
                pc: u32::MAX,
                level: u8::MAX,
                ra: Some(u32::MAX),
            },
            MAX_PAGE_SAMPLES,
        )
        .collect(),
    });
    assert!(postcard::to_slice(&samples, &mut buffer).unwrap().len() <= 512);
}

fn full_cache() -> CacheCounts {
    let instruction = InstructionBus {
        hits: u32::MAX,
        misses: u32::MAX,
        conflicts: u32::MAX,
        next_level_reads: u32::MAX,
        overflowed: true,
    };
    let data = DataBus {
        hits: u32::MAX,
        misses: u32::MAX,
        conflicts: u32::MAX,
        next_level_reads: u32::MAX,
        next_level_writes: u32::MAX,
        overflowed: true,
    };
    CacheCounts {
        ibus0: instruction,
        ibus1: instruction,
        dbus0: data,
        dbus1: data,
    }
}

fn full_geometry() -> CacheGeometry {
    let level = CacheLevel {
        size_bytes: Some(u32::MAX),
        line_bytes: Some(u32::MAX),
        ways: Some(u32::MAX),
    };
    CacheGeometry {
        l1_icache: level,
        l1_dcache: level,
        l2: level,
        l2_bypassed: true,
    }
}

#[test]
fn the_window_s_cache_counts_and_geometry_round_trip() {
    let counters = Counters {
        cycles: 1_600_000_000,
        instructions: 320_000_000,
        stack_received: 4000,
        cache: CacheCounts {
            ibus0: InstructionBus {
                hits: 250_000_000,
                misses: 1_200_000,
                conflicts: 7,
                next_level_reads: 1_200_000,
                overflowed: false,
            },
            dbus1: DataBus {
                next_level_writes: 3,
                overflowed: true,
                ..DataBus::default()
            },
            ..CacheCounts::default()
        },
        cache_geometry: CacheGeometry {
            l1_icache: CacheLevel {
                size_bytes: Some(32 * 1024),
                line_bytes: Some(64),
                ways: Some(2),
            },
            l2: CacheLevel {
                size_bytes: Some(256 * 1024),
                line_bytes: None,
                ways: None,
            },
            l2_bypassed: true,
            ..CacheGeometry::default()
        },
        ..Counters::default()
    };
    let mut buffer = [0; 512];
    for message in [
        CountersCollected::Report(counters),
        CountersCollected::Report(Counters {
            cache: full_cache(),
            cache_geometry: full_geometry(),
            ..counters
        }),
    ] {
        let encoded = postcard::to_slice(&message, &mut buffer).unwrap();
        assert_eq!(
            postcard::from_bytes::<CountersCollected>(encoded).unwrap(),
            message
        );
    }
}

#[test]
fn the_service_names_six_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 12);
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len());
}

#[test]
fn a_tally_remembers_every_sequence_of_a_bounded_run() {
    use tally::{Arrival, Tally};
    let mut tally = Tally::<4>::default();
    // The review's counterexample: an old duplicate far behind the highest.
    assert_eq!(tally.count(0, 10), Arrival::New);
    assert_eq!(tally.count(128, 10), Arrival::New);
    assert_eq!(tally.count(0, 10), Arrival::Duplicate);
    assert_eq!((tally.datagrams, tally.bytes, tally.duplicates), (2, 20, 1));
    assert_eq!(tally.lost(), 127);
    assert_eq!(tally.count(5, 10), Arrival::Late);
    assert_eq!(tally.reordered, 1);
    assert_eq!(tally.lost(), 126);
    // Past the capacity: neither distinct nor a duplicate.
    assert_eq!(Tally::<4>::CAPACITY, 256);
    assert_eq!(tally.count(256, 10), Arrival::Untracked);
    assert_eq!(tally.count(256, 10), Arrival::Untracked);
    assert_eq!((tally.datagrams, tally.untracked), (3, 2));
    assert_eq!(tally.highest, Some(128));
}

#[test]
fn a_cleared_tally_counts_the_next_run_afresh() {
    use tally::{Arrival, Tally};
    let mut tally = Tally::<4>::new();
    assert_eq!(tally, Tally::default());
    tally.count(3, 10);
    tally.count(3, 10);
    tally.count(300, 10);
    tally.clear();
    assert_eq!(tally, Tally::new());
    assert_eq!(
        tally.count(3, 10),
        Arrival::New,
        "no sequence is remembered"
    );
}
