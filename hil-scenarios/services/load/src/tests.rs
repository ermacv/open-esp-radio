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
        rx: full_rx(),
        tx: TxCounts {
            ampdu: TxAmpdu {
                aggregates: u32::MAX,
                refused: u32::MAX,
                subframes: u32::MAX,
                subframes_1: u16::MAX,
                subframes_2: u16::MAX,
                subframes_3_4: u16::MAX,
                subframes_5_8: u16::MAX,
                subframes_9_16: u16::MAX,
                subframes_17_32: u16::MAX,
                acknowledged: u32::MAX,
                retried: u32::MAX,
                dropped: u32::MAX,
                block_ack_requests: u32::MAX,
            },
        },
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
    let link = LinkCollected::Report(full_link());
    let encoded = postcard::to_slice(&link, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<LinkCollected>(encoded).unwrap(),
        link
    );
}

/// Every link count at its largest.
fn full_link() -> Link {
    let all = u32::MAX;
    let rates = Rates {
        ht20_long_gi: [all; 8],
        ht20_short_gi: [all; 8],
        ht40_long_gi: [all; 8],
        ht40_short_gi: [all; 8],
        ht_other: all,
        non_ht: all,
        he: all,
        unknown: all,
    };
    Link {
        rx: RxLink {
            mpdus: rates,
            in_ampdu: all,
            single: all,
            containment_unknown: all,
        },
        tx: TxLink {
            mpdus: rates,
            transmissions: all,
            delivered: all,
            ack_timeouts: all,
            other_ends: all,
            unsent: all,
        },
    }
}

#[test]
fn a_link_window_takes_wrapping_differences_of_every_count() {
    let mut start = Link::default();
    start.rx.mpdus.ht40_short_gi[7] = u32::MAX;
    start.rx.in_ampdu = 10;
    start.tx.transmissions = 100;
    start.tx.mpdus.non_ht = 1;
    let mut end = start;
    end.rx.mpdus.ht40_short_gi[7] = 9;
    end.rx.mpdus.ht20_long_gi[0] = 2;
    end.rx.in_ampdu = 25;
    end.tx.transmissions = 130;
    end.tx.delivered = 20;
    end.tx.mpdus.non_ht = 4;
    let window = Link::window(&start, &end);
    assert_eq!(window.rx.mpdus.ht40_short_gi, [0, 0, 0, 0, 0, 0, 0, 10]);
    assert_eq!(window.rx.mpdus.ht20_long_gi[0], 2);
    assert_eq!(window.rx.in_ampdu, 15);
    assert_eq!(
        (
            window.tx.transmissions,
            window.tx.delivered,
            window.tx.mpdus.non_ht
        ),
        (30, 20, 3)
    );
    assert_eq!(Link::window(&end, &end), Link::default());
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
            rx: full_rx(),
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

/// Every receive count at its largest.
fn full_rx() -> RxCounts {
    let all = u32::MAX;
    RxCounts {
        driver: DriverRx {
            available: true,
            mac_irqs: all,
            units: all,
            bytes: all,
            overload_drops: all,
            admission_blocks: all,
            stage_capacity_blocks: all,
            chained_drops: all,
            oversized_drops: all,
            empty_drops: all,
            too_long_drops: all,
            service_passes: all,
            service_errors: all,
            capacity_drains: all,
            continued_immediately: all,
            continued_after_wait: all,
            rx_failures: all,
            mic_failures: all,
            quarantined: all,
            prefix_other: all,
        },
        station: StationRx {
            port_events_lost: all,
            dropped_header: all,
            dropped_frame_kind: all,
            dropped_address: all,
            dropped_fragmented: all,
            dropped_protection: all,
            dropped_integrity: all,
            dropped_replay: all,
            dropped_payload: all,
            dropped_not_connected: all,
            dropped_security: all,
            dropped_draining: all,
            refused_too_long: all,
            refused_queue_full: all,
            refused_no_network: all,
            copied_aggregated: all,
            copied_kept: all,
            mpdus_in_ampdu: all,
            mpdus_single: all,
            mpdus_unknown: all,
            released_in_order: all,
            released_by_timeout: all,
            released_by_bar: all,
            released_by_stop: all,
            missing_by_timeout: all,
            missing_other: all,
            duplicates: all,
            rejected: all,
            agreements_started: all,
            agreements_stopped: all,
            agreement_window: u16::MAX,
            rsn_inputs_dropped: all,
        },
    }
}

#[test]
fn a_receive_window_takes_wrapping_differences_and_the_end_s_agreement_window() {
    let start = RxCounts {
        driver: DriverRx {
            available: true,
            units: u32::MAX,
            overload_drops: 3,
            ..DriverRx::default()
        },
        station: StationRx {
            released_by_timeout: 10,
            agreement_window: 32,
            ..StationRx::default()
        },
    };
    let end = RxCounts {
        driver: DriverRx {
            available: true,
            units: 4,
            overload_drops: 9,
            ..DriverRx::default()
        },
        station: StationRx {
            released_by_timeout: 12,
            agreement_window: 16,
            ..StationRx::default()
        },
    };
    let driver = DriverRx::window(&start.driver, &end.driver);
    assert_eq!((driver.units, driver.overload_drops), (5, 6));
    assert!(driver.available);
    let station = StationRx::window(&start.station, &end.station);
    assert_eq!(
        (station.released_by_timeout, station.agreement_window),
        (2, 16)
    );
    // A reading the image could not take makes the window's unavailable.
    assert!(!DriverRx::window(&DriverRx::default(), &end.driver).available);
}

#[test]
fn a_transmit_window_takes_wrapping_differences() {
    let start = TxAmpdu {
        aggregates: u32::MAX,
        subframes_17_32: 2,
        block_ack_requests: 1,
        ..TxAmpdu::default()
    };
    let end = TxAmpdu {
        aggregates: 3,
        subframes_17_32: 6,
        block_ack_requests: 1,
        ..TxAmpdu::default()
    };
    let window = TxAmpdu::window(&start, &end);
    assert_eq!(
        (
            window.aggregates,
            window.subframes_17_32,
            window.block_ack_requests
        ),
        (4, 4, 0)
    );
}

#[test]
fn the_service_names_seven_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 14);
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
