extern crate std;

use super::header::{Header, LENGTH};
use super::*;

#[test]
fn start_barrier_messages_round_trip_with_device_boundaries() {
    let begin = Begin {
        run: u32::MAX,
        receive: true,
    };
    let ready = Begun::Ready {
        at_us: u64::MAX - 1,
        first_received_us: Some(u64::MAX - 2),
        window_start_us: u64::MAX,
    };
    let mut buffer = [0; 128];
    let wire = postcard::to_slice(&begin, &mut buffer).unwrap();
    assert_eq!(postcard::from_bytes::<Begin>(wire).unwrap(), begin);
    for reply in [
        ready,
        Begun::WaitingForLoad,
        Begun::Refused(LoadError::UnknownRun),
    ] {
        let wire = postcard::to_slice(&reply, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<Begun>(wire).unwrap(), reply);
    }
}

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
fn the_header_s_octets_are_what_it_writes_and_reads_back() {
    let header = Header {
        run: 7,
        sequence: 0x0102_0304_0506_0708,
        sent_us: 42,
        echo_request: true,
        echo_reply: false,
    };
    let mut payload = [0xa5; 64];
    header.write(&mut payload).unwrap();
    assert_eq!(payload[..LENGTH], header.to_bytes());
    assert!(payload[LENGTH..].iter().all(|&octet| octet == 0xa5));
    assert_eq!(Header::read(&header.to_bytes()), Some(header));
}

#[test]
fn the_header_s_octets_follow_its_table() {
    let header = Header {
        run: 0x0a0b_0c0d,
        sequence: 0x1011_1213_1415_1617,
        sent_us: 0x2021_2223_2425_2627,
        echo_request: false,
        echo_reply: true,
    };
    let expected: [u8; LENGTH] = [
        b'O', b'E', b'R', b'L', 2, 0, 0, 0, 0x0a, 0x0b, 0x0c, 0x0d, 0x10, 0x11, 0x12, 0x13, 0x14,
        0x15, 0x16, 0x17, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
    ];
    assert_eq!(header.to_bytes(), expected);
    let both = Header {
        echo_request: true,
        ..header
    };
    assert_eq!(both.to_bytes()[4], 3);
}

#[test]
fn the_largest_reports_fit_one_response() {
    let load = Load {
        now_us: u64::MAX,
        harts: (0..MAX_HARTS as u8)
            .map(|hart| HartLoad {
                hart,
                idle_us: u64::MAX,
                wakes: u32::MAX,
            })
            .collect(),
        executors: (0..MAX_EXECUTORS as u8)
            .map(|executor| ExecutorLoad {
                executor,
                hart: 1,
                level: 3,
                work_us: u64::MAX,
                rounds: u32::MAX,
            })
            .collect(),
    };
    let mut buffer = [0; 1024];
    assert!(postcard::to_slice(&load, &mut buffer).unwrap().len() <= 256);
    let report = Collected::Report(Report {
        armed_us: u64::MAX,
        triggered_us: u64::MAX,
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
    let tracking = TrackingCollected::Report(Tracking {
        available: true,
        ticks: u32::MAX,
        not_due: u32::MAX,
        unavailable: u32::MAX,
        inhibited: u32::MAX,
        rfpll_corrected: u32::MAX,
        wifi_i2c_changed: u32::MAX,
        wifi_tx_power: u32::MAX,
        bluetooth_ieee802154_tx_power: u32::MAX,
        calibration_common: u32::MAX,
        calibration_transmit: u32::MAX,
        temperature_last: Some(i16::MIN),
        temperature_min: Some(i16::MIN),
        temperature_max: Some(i16::MAX),
        tick_us: u32::MAX,
        longest_tick_us: Some(u32::MAX),
    });
    let encoded = postcard::to_slice(&tracking, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<TrackingCollected>(encoded).unwrap(),
        tracking
    );
    let network = NetworkCollected::Report(full_network());
    let encoded = postcard::to_slice(&network, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<NetworkCollected>(encoded).unwrap(),
        network
    );
    let mac = MacCollected::Report(Mac::new([u32::MAX; MAC_COUNTS], i32::MIN));
    assert!(postcard::to_slice(&mac, &mut buffer).unwrap().len() <= 512);
    let stall = StallCollected::Report(full_stall());
    let encoded = postcard::to_slice(&stall, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<StallCollected>(encoded).unwrap(),
        stall
    );
    let full = Mac::new([u32::MAX; MAC_COUNTS], i32::MIN);
    let banks = StallMacCollected::Report(StallMac {
        fresh: Some(full),
        periodic: Some(full),
        periodic_us: Some(u64::MAX),
    });
    let encoded = postcard::to_slice(&banks, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<StallMacCollected>(encoded).unwrap(),
        banks
    );
    let link = LinkCollected::Report(full_link());
    let encoded = postcard::to_slice(&link, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<LinkCollected>(encoded).unwrap(),
        link
    );
}

/// A stall snapshot with every value at its largest.
fn full_stall() -> Stall {
    let all = u32::MAX;
    let boundary = StallBoundary {
        count: all,
        last_us: Some(u64::MAX),
    };
    Stall {
        requested_us: u64::MAX,
        socket_last_us: Some(u64::MAX),
        socket_after_us: Some(u64::MAX),
        snapshot: Some(RxSnapshot {
            at_us: u64::MAX,
            dma: boundary,
            driver: boundary,
            device: StallDevice {
                rx_pending: true,
                continuation: true,
                cut: true,
                pausing: true,
                paused: true,
                queued: all,
            },
            hardware: Some(StallHardware {
                walker: StallWalker {
                    enabled: true,
                    reload_doorbell: true,
                    next_low: all,
                    next_upper: true,
                    last_low: all,
                    base: Some(all),
                    buffer_full: Some(u16::MAX),
                    runtime_state: u8::MAX,
                },
                ring: Some(StallRing {
                    base: all,
                    recycle_start: all,
                    accepted_tail: all,
                    observed: all,
                    completed: all,
                    remaining: Some(all),
                    reload_pending: true,
                }),
                session: StallSession {
                    descriptors: all,
                    armed: all,
                    done: all,
                    software: all,
                    reserved_owner: true,
                    reserved_done: true,
                    released: all,
                    detached: all,
                    staged: all,
                    masked: true,
                    frozen: true,
                    line_request_pending: true,
                    receive_fact_pending: Some(true),
                    mac_irqs: all,
                    doorbell_unmasks: all,
                    ignored_unmasks: all,
                    service_passes: all,
                },
            }),
            resume: Some(StallResume {
                checked_us: u64::MAX,
                dma: boundary,
                driver: boundary,
                serviced: true,
            }),
        }),
    }
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
            missing: [all; 7],
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
            refused_pressure: all,
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
fn a_mac_count_wraps_at_its_own_width() {
    let at = |name| {
        MAC_COUNT_NAMES
            .iter()
            .position(|(known, _)| *known == name)
            .unwrap()
    };
    let (mut start, mut end) = ([0; MAC_COUNTS], [0; MAC_COUNTS]);
    start[at("fcs_error")] = 0xfffe;
    end[at("fcs_error")] = 1;
    start[at("nrx")] = 0x3ff;
    end[at("nrx")] = 2;
    start[at("tx_rts")] = u32::MAX;
    let window = Mac::window(&Mac::new(start, 7), &Mac::new(end, -40));
    assert_eq!(window.count(at("fcs_error")), 3);
    assert_eq!(window.count(at("nrx")), 3);
    assert_eq!(window.count(at("tx_rts")), 1);
    assert_eq!(window.frequency_offset, -40);
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
fn the_service_names_nineteen_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_protocols_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 38);
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

#[test]
fn a_positions_window_takes_wrapping_differences_and_fits_one_response() {
    let start = Positions {
        sent: [u32::MAX; 32],
        interior: u32::MAX,
        ..Positions::default()
    };
    let mut end = Positions {
        interior: 1,
        ..Positions::default()
    };
    end.sent[3] = 1;
    end.tail_runs[2] = 4;
    let window = Positions::window(&start, &end);
    assert_eq!((window.sent[3], window.sent[0]), (2, 1));
    assert_eq!((window.tail_runs[2], window.interior), (4, 2));
    let mut buffer = [0; 1024];
    let largest = PositionsCollected::Report(Positions {
        sent: [u32::MAX; 32],
        holes: [u32::MAX; 32],
        tail_runs: [u32::MAX; 32],
        interior: u32::MAX,
    });
    assert!(postcard::to_slice(&largest, &mut buffer).unwrap().len() <= 512);
}

#[test]
fn hart_1_s_two_ends_round_trip_and_fit_one_response() {
    let largest = HartCount {
        cycles: u64::MAX,
        instructions: u64::MAX,
    };
    let message = Hart1Collected::Report(Hart1Window {
        start: largest,
        end: largest,
    });
    let mut buffer = [0; 512];
    let encoded = postcard::to_slice(&message, &mut buffer).unwrap();
    assert_eq!(
        postcard::from_bytes::<Hart1Collected>(encoded).unwrap(),
        message
    );
}

#[test]
fn the_addba_requests_round_trip_every_outcome_and_fit_one_response() {
    let outcomes = [
        AddbaOutcome::Accepted { window: u16::MAX },
        AddbaOutcome::Repeated { window: u16::MAX },
        AddbaOutcome::Refused { status: u16::MAX },
        AddbaOutcome::Unanswered,
    ];
    let mut last = [None; ADDBA_TIDS];
    for (tid, request) in last.iter_mut().enumerate() {
        *request = Some(AddbaRequest {
            requested: u16::MAX,
            outcome: outcomes[tid % outcomes.len()],
        });
    }
    let largest = AddbaCollected::Report(Addba {
        last,
        accepted: u32::MAX,
        repeated: u32::MAX,
        refused: u32::MAX,
        unanswered: u32::MAX,
    });
    let mut buffer = [0; 1024];
    let encoded = postcard::to_slice(&largest, &mut buffer).unwrap();
    assert!(encoded.len() <= 512, "{}", encoded.len());
    assert_eq!(
        postcard::from_bytes::<AddbaCollected>(encoded).unwrap(),
        largest
    );
    let requested_zero = AddbaCollected::Report(Addba {
        last: core::array::from_fn(|tid| {
            (tid == 0).then_some(AddbaRequest {
                requested: 0,
                outcome: AddbaOutcome::Accepted { window: 16 },
            })
        }),
        accepted: 1,
        ..Addba::default()
    });
    let encoded = postcard::to_slice(&requested_zero, &mut buffer).unwrap();
    assert_eq!(
        postcard::from_bytes::<AddbaCollected>(encoded).unwrap(),
        requested_zero
    );
}

/// Every turn count at its largest.
fn full_network() -> NetworkTurns {
    let all = u32::MAX;
    NetworkTurns {
        available: true,
        turns: all,
        reserves: all,
        releases: all,
        sends: all,
        closes: all,
        budget_spent: all,
        budget_turns: all,
        intake_full: all,
        accepted: all,
        accepted_by_turn: [all; ACCEPTED_BINS],
        publish_chose: all,
        publish_refused: all,
        publish_budget: all,
        notices_full: all,
        publish_fault: all,
        blocks_out: all,
        credits_out: all,
        yields: all,
        yields_with_work: all,
        waits_pending: all,
        waits_ready: all,
        reserve_blocks: all,
        reserve_by_size: [all; RESERVE_BINS],
        local_offered: all,
        local_refused: all,
        local_held: all,
        local_declined: all,
        local_withdrawn: all,
        local_waits_offers: all,
        local_waits_changes: all,
        producer_wakes: all,
        producer_unknown: all,
        drops: [all; DROP_KINDS],
        loans_at_turn_end: [all; LOAN_BINS],
    }
}

#[test]
fn a_network_window_is_each_count_s_difference_and_the_end_s_availability() {
    let start = NetworkTurns {
        available: true,
        turns: u32::MAX,
        sends: 10,
        accepted_by_turn: [1, 2, 3, 4, 5, 6, u32::MAX],
        reserve_blocks: 30,
        reserve_by_size: [u32::MAX, 0, 1, 2, 3],
        local_offered: 5,
        producer_wakes: u32::MAX,
        drops: core::array::from_fn(|kind| if kind == 11 { u32::MAX } else { 1 }),
        loans_at_turn_end: [3, 0, 0, 0, 1, 0],
        ..NetworkTurns::default()
    };
    let end = NetworkTurns {
        available: true,
        turns: 4,
        sends: 18,
        accepted_by_turn: [1, 3, 6, 4, 5, 6, 0],
        waits_pending: 2,
        reserve_blocks: 62,
        reserve_by_size: [0, 0, 1, 2, 5],
        local_offered: 9,
        local_waits_changes: 3,
        producer_wakes: 1,
        drops: core::array::from_fn(|kind| if kind == 11 { 6 } else { 1 }),
        loans_at_turn_end: [5, 0, 0, 0, 9, 0],
        ..NetworkTurns::default()
    };
    let window = NetworkTurns::window(&start, &end);
    assert_eq!(window.turns, 5, "a count wraps at 2^32");
    assert_eq!(window.sends, 8);
    assert_eq!(window.accepted_by_turn, [0, 1, 3, 0, 0, 0, 1]);
    assert_eq!(window.waits_pending, 2);
    assert_eq!(window.reserve_blocks, 32);
    assert_eq!(window.reserve_by_size, [1, 0, 0, 0, 2]);
    assert_eq!(
        (
            window.local_offered,
            window.local_waits_changes,
            window.producer_wakes
        ),
        (4, 3, 2)
    );
    assert_eq!(
        window.drops,
        core::array::from_fn(|kind| u32::from(kind == 11) * 7),
        "each reason's own difference, wrapping"
    );
    assert_eq!(window.loans_at_turn_end, [2, 0, 0, 0, 8, 0]);
    assert!(window.available);
    assert!(!NetworkTurns::window(&start, &NetworkTurns::default()).available);
}

#[test]
fn a_network_request_round_trips() {
    let mut buffer = [0; 64];
    let request = CollectNetwork { run: u32::MAX };
    let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
    assert_eq!(
        postcard::from_bytes::<CollectNetwork>(encoded).unwrap(),
        request
    );
    for answer in [
        NetworkCollected::Running,
        NetworkCollected::Refused(LoadError::UnknownRun),
    ] {
        let encoded = postcard::to_slice(&answer, &mut buffer).unwrap();
        assert_eq!(
            postcard::from_bytes::<NetworkCollected>(encoded).unwrap(),
            answer
        );
    }
}

#[test]
fn the_diagnostics_requests_and_answers_round_trip_and_fit_one_response() {
    let mut buffer = [0; 512];
    for request in [
        Diagnose {
            on: true,
            reset: false,
        },
        Diagnose {
            on: false,
            reset: true,
        },
    ] {
        let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<Diagnose>(encoded).unwrap(), request);
    }
    for reset in [Reset::NotAsked, Reset::Asked, Reset::Refused] {
        let answer = Diagnosed {
            was_on: true,
            reset,
        };
        let encoded = postcard::to_slice(&answer, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<Diagnosed>(encoded).unwrap(), answer);
    }
    let request = CollectDiagnostics { run: u32::MAX };
    let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
    assert_eq!(
        postcard::from_bytes::<CollectDiagnostics>(encoded).unwrap(),
        request
    );
    let full = NetworkDiagnostics {
        turns: u32::MAX,
        walked: u32::MAX,
        told: u32::MAX,
        return_batches: u32::MAX,
        blocks_returned: u32::MAX,
        credits_restored: u32::MAX,
        no_credit: u32::MAX,
        no_credit_loans: u32::MAX,
        no_credit_unreturned: u32::MAX,
        materialised: u32::MAX,
        stage_cycles: [u64::MAX; DIAGNOSTIC_STAGES],
    };
    for answer in [
        DiagnosticsCollected::Report(full),
        DiagnosticsCollected::NoWindow,
        DiagnosticsCollected::Exhausted,
        DiagnosticsCollected::Running,
        DiagnosticsCollected::Refused(LoadError::UnknownRun),
    ] {
        let encoded = postcard::to_slice(&answer, &mut buffer).unwrap();
        assert!(encoded.len() <= 512, "{}", encoded.len());
        assert_eq!(
            postcard::from_bytes::<DiagnosticsCollected>(encoded).unwrap(),
            answer
        );
    }
}

#[test]
fn every_profile_page_at_its_largest_fits_one_response() {
    let mut buffer = [0; 1024];
    let cell = TurnCell {
        turns: u32::MAX,
        cycles: u64::MAX,
        items: u32::MAX,
    };
    let pages = [
        ProfileCollected::Moments(TurnMoments {
            available: true,
            moments: [u64::MAX; PROFILE_MOMENTS],
            empty: [u64::MAX; 3],
            idle: [u64::MAX; 2],
            gaps: [[u64::MAX; 2]; PROFILE_GAPS],
        }),
        ProfileCollected::Cells(TurnCells {
            receive_only: [cell; PROFILE_ITEM_CELLS],
            transmit_only: [cell; PROFILE_ITEM_CELLS],
            notice_only: [cell; PROFILE_NOTICE_CELLS],
        }),
        ProfileCollected::Spread(TurnSpread {
            cycles_by_log2: [u32::MAX; PROFILE_CYCLE_BINS],
            notices_by_turn: [u32::MAX; PROFILE_NOTICE_BINS],
            frames_by_turn: [u32::MAX; PROFILE_FRAME_BINS],
            settling: [u32::MAX; 3],
            returns: ReturnBursts {
                bursts: u32::MAX,
                queued: u32::MAX,
                queued_by_burst: [u32::MAX; RETURN_BURST_BINS],
                before_by_burst: [u32::MAX; RETURN_OCCUPANCY_BINS],
                after_by_burst: [u32::MAX; RETURN_OCCUPANCY_BINS],
                held_bursts: u32::MAX,
                held: u32::MAX,
            },
        }),
        ProfileCollected::Running,
        ProfileCollected::Refused(LoadError::InvalidParameters),
    ];
    for page in pages {
        let encoded = postcard::to_slice(&page, &mut buffer).unwrap();
        assert!(encoded.len() <= 512, "{page:?}: {}", encoded.len());
        assert_eq!(
            postcard::from_bytes::<ProfileCollected>(encoded).unwrap(),
            page
        );
    }
    let request = CollectProfile {
        run: u32::MAX,
        page: u8::MAX,
    };
    let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
    assert_eq!(
        postcard::from_bytes::<CollectProfile>(encoded).unwrap(),
        request
    );
}
