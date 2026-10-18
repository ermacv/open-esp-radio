use super::report::project_mac_units;
use super::*;

#[test]
fn cross_device_mac_projection_ignores_missing_tid() {
    let mut units = BTreeMap::new();
    units.insert(
        MacFrameKey {
            tid: u8::MAX,
            sequence: 42,
            fragment: 0,
        },
        2,
    );
    units.insert(
        MacFrameKey {
            tid: 3,
            sequence: 42,
            fragment: 0,
        },
        1,
    );
    units.insert(
        MacFrameKey {
            tid: 0,
            sequence: 43,
            fragment: 1,
        },
        4,
    );

    assert_eq!(project_mac_units(&units).get(&(42, 0)), Some(&3));
    assert_eq!(project_mac_units(&units).get(&(43, 1)), Some(&4));
}

#[test]
fn air_correlation_matches_ap_mpdus_seen_by_the_observer() {
    let key = |tid, sequence| MacFrameKey {
        tid,
        sequence,
        fragment: 0,
    };
    let ap = OpenWrtTxMonitorEvidence {
        mac_units: BTreeMap::from([(key(u8::MAX, 1), 2), (key(u8::MAX, 2), 1)]),
        ..OpenWrtTxMonitorEvidence::default()
    };
    let observer = LocalAirMonitorEvidence {
        mac_units: BTreeMap::from([(key(0, 1), 1), (key(0, 9), 1)]),
        ..LocalAirMonitorEvidence::default()
    };
    let correlation = AirCorrelation::of(&ap, &observer);
    assert_eq!(correlation.matched, 1);
    assert_eq!(correlation.ap_not_observed, 2);
    assert_eq!(correlation.observer_extra, 1);
    assert_eq!(correlation.ap_known_tid, 0);
    assert_eq!(correlation.observer_known_tid, 2);
}

#[test]
fn extracts_tx_task_polls_without_an_rx_interval_marker() {
    let task_polls = task_polls_from_log(
        "ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_rx polls=0 poll_us=0 poll_boot_max_us=90 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_tx polls=5000 poll_us=125000 poll_boot_max_us=95 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=tcp polls=0 poll_us=0 poll_boot_max_us=80 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n",
    );

    assert!(task_polls.is_complete());
    assert_eq!(task_polls.network.polls, 5_100);
    assert_eq!(task_polls.radio.poll_us, 390_000);
    assert_eq!(task_polls.udp_tx.poll_us, 125_000);
}

#[test]
fn task_polls_become_typed_measurements_per_reporting_task() {
    let measurements =
        task_polls_from_log("ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n")
            .measurements();
    let value = |name: &str| {
        measurements
            .iter()
            .find(|measurement| measurement.name == name)
            .map(|measurement| (measurement.value, measurement.unit))
    };
    use oer_hil_run_bundle_format::run::MeasurementUnit;
    assert_eq!(
        value("task_poll.radio.poll_us"),
        Some((390_000, MeasurementUnit::Microseconds))
    );
    assert_eq!(
        value("task_poll.radio.over_100us"),
        Some((20, MeasurementUnit::Count))
    );
    // A task that reported no interval publishes nothing.
    assert_eq!(value("task_poll.network.polls"), None);
    assert_eq!(measurements.len(), 7);
}

#[test]
fn ht40_tx_accepts_mcs7_long_or_short_gi_but_not_a_lower_vector() {
    let mut report = DeviceReport {
        tx: vec![TxSample {
            throughput_kbps: 80_000,
            bandwidth_mhz: 40,
            rate_kbps: 135_000,
        }],
        ..DeviceReport::default()
    };
    assert_eq!(qualify_tx_samples(&report, 40, 135_000).unwrap(), 80_000);

    report.tx[0].rate_kbps = 150_000;
    assert_eq!(qualify_tx_samples(&report, 40, 135_000).unwrap(), 80_000);

    report.tx[0].rate_kbps = 121_500;
    assert!(qualify_tx_samples(&report, 40, 135_000).is_err());
    report.tx[0].rate_kbps = 135_000;
    report.tx[0].bandwidth_mhz = 20;
    assert!(qualify_tx_samples(&report, 40, 135_000).is_err());
}

#[test]
fn ht40_rx_vector_gate_covers_the_complete_interval_and_guard_policy() {
    let mut radio = RxRadioEvidence {
        not_s_mpdu_datagrams: 100,
        ..RxRadioEvidence::default()
    };
    radio.ht40_long_gi_frames = 100;
    validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Long).unwrap();
    validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Any).unwrap();
    assert!(validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Short,).is_err());

    radio.ht40_below_mcs7_frames = 1;
    assert!(validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Any).is_err());

    radio.ht40_below_mcs7_frames = 0;
    radio.ht40_long_gi_frames = 99;
    radio.ht_invalid_frames = 1;
    validate_ht40_rx_vector(&radio, None, HtGuardIntervalExpectation::Any).unwrap();
    assert!(validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Any).is_err());

    radio.ht_invalid_frames = 0;
    assert!(
        validate_ht40_rx_vector(&radio, Some(7), HtGuardIntervalExpectation::Any).is_err(),
        "missing one vector must fail instead of being treated as sampling",
    );

    radio.ht40_long_gi_frames = 0;
    radio.ht_invalid_frames = 100;
    assert!(validate_ht40_rx_vector(&radio, None, HtGuardIntervalExpectation::Any).is_err());
}

#[test]
fn quantifies_delivery_loss_after_positive_block_ack() {
    let ampdu = AmpduEvidence {
        subframes: 31_101,
        acknowledged: 31_073,
        ..AmpduEvidence::default()
    };
    let host = Burst {
        datagrams: 31_005,
        missing: 96,
        ..Burst::default()
    };

    assert_eq!(
        post_block_ack_delivery_loss_lower_bound(ampdu, host, 31_101),
        Some(68),
    );
}

#[test]
fn post_block_ack_comparison_requires_one_subframe_per_typed_tx_unit() {
    let ampdu = AmpduEvidence {
        subframes: 31_100,
        acknowledged: 31_073,
        ..AmpduEvidence::default()
    };
    let host = Burst {
        datagrams: 31_005,
        ..Burst::default()
    };

    assert_eq!(
        post_block_ack_delivery_loss_lower_bound(ampdu, host, 31_101),
        None,
    );
}

#[test]
fn exact_rx_delivery_rejects_loss_and_reordering() {
    let exact = UdpSequenceEvidence {
        intervals: 1,
        first: Some(0),
        highest: Some(3),
        next: Some(4),
        ..UdpSequenceEvidence::default()
    };
    assert!(validate_exact_rx_delivery(4, 4, exact, RxOrderEvidence::default()).is_ok());

    let missing = UdpSequenceEvidence {
        gap_events: 1,
        forward_missing: 1,
        ..exact
    };
    assert!(validate_exact_rx_delivery(4, 3, missing, RxOrderEvidence::default()).is_err());

    let reordered = UdpSequenceEvidence {
        gap_events: 1,
        forward_missing: 1,
        backward: 1,
        ..exact
    };
    assert!(validate_exact_rx_delivery(4, 4, reordered, RxOrderEvidence::default()).is_err());
}

#[test]
fn localizes_recovered_udp_reordering_across_rx_boundaries() {
    let sequence = UdpSequenceEvidence {
        first: Some(0),
        highest: Some(3),
        next: Some(4),
        gap_events: 1,
        forward_missing: 1,
        backward: 1,
        ..UdpSequenceEvidence::default()
    };
    let before_mac = RxOrderEvidence {
        intervals: 1,
        gap_events: 1,
        forward_missing: 1,
        backward: 1,
        backward_mac_forward: 1,
        ..RxOrderEvidence::default()
    };
    let error = validate_exact_rx_delivery(4, 4, sequence, before_mac)
        .unwrap_err()
        .to_string();
    assert!(error.contains("predates the open driver's per-TID MAC/BlockAck boundary"));

    let after_handoff = RxOrderEvidence {
        intervals: 1,
        ..RxOrderEvidence::default()
    };
    let error = validate_exact_rx_delivery(4, 4, sequence, after_handoff)
        .unwrap_err()
        .to_string();
    assert!(error.contains("after the pre-network ConnectedRx observer"));
}

#[test]
fn excludes_readiness_probe_health_from_sustained_rx_evidence() {
    let report = parse_device_report(
        "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=64 datagrams=1 elapsed_us=1 throughput_kbps=512000 code_address=1342257664\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=54 queue_dropped=1 rx_format=4\n\
             ORXQ first=0 highest=0 next=1 gap_events=0 forward_missing=0 maximum_gap=0 maximum_gap_at=4294967295 first_gap_at=4294967295 last_gap_at=4294967295 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=0 maximum_interarrival_at=4294967295\n\
             ORXO gap_events=0 forward_missing=0 backward=0 adjacent_duplicates=0 backward_mac_backward=0 backward_mac_same=0 backward_mac_forward=0 backward_mac_other_tid=0 backward_mac_unavailable=0\n\
             ORXSM s_mpdu=0 not_s_mpdu=1 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=1 beacon_unavailable=0\n\
             ORXAG ampdu=1 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=1 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=3 frontier=37 admitted=37 bytes=60860 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=1 queue_floor=1 fmax=31 amax=31 service_us=636 service_boot_max_us=503\n\
             ORXSC samples=3 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=1 queue_floor=1\n\
             ORTP task=network polls=2 poll_us=1626 poll_boot_max_us=1582 over_100us=1 over_500us=1 over_1000us=1 over_5000us=0\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=6000000 datagrams=5000 elapsed_us=5000000 throughput_kbps=9600 code_address=1342257664\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_format=4\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=2 forward_missing=3 maximum_gap=2 maximum_gap_at=100 first_gap_at=100 last_gap_at=3000 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=250 maximum_interarrival_at=100\n\
             ORXO gap_events=2 forward_missing=3 backward=3 adjacent_duplicates=0 backward_mac_backward=3 backward_mac_same=0 backward_mac_forward=0 backward_mac_other_tid=0 backward_mac_unavailable=0\n\
             ORXSM s_mpdu=100 not_s_mpdu=4900 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXSC samples=5000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63\n\
             ORXL transactions=300 reload_us=1500 reload_boot_max_us=9\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n",
    );

    assert_eq!(report.software_health, [(5_000, 0)]);
    assert_eq!(report.dma_health, [(0, 0)]);
    assert_eq!(report.rx_service.len(), 3);
    assert_eq!(report.rx_service[0].service_calls, 5_000);
    assert_eq!(report.rx_service[1].service_credit_samples, 5_000);
    assert_eq!(report.rx_service[1].minimum_pool_credits, 31);
    assert_eq!(report.rx_service[1].minimum_queue_credits, 63);
    assert_eq!(report.rx_service[2].reload_transactions, 300);
    assert_eq!(report.rx_service[2].reload_us, 1_500);
    assert_eq!(report.rx_service[2].reload_max_us, 9);
    assert_eq!(report.task_polls.network.intervals, 1);
    assert_eq!(report.task_polls.network.polls, 5_100);
    assert_eq!(report.rx_sequences.len(), 1);
    assert_eq!(report.rx_sequences[0].forward_missing, 3);
    assert_eq!(report.rx_order.len(), 1);
    assert_eq!(report.rx_order[0].backward_mac_backward, 3);
    assert_eq!(report.rx_s_mpdu.len(), 1);
    assert_eq!(report.rx_s_mpdu[0].not_s_mpdu_datagrams, 4_900);
    assert_eq!(report.rx_s_mpdu[0].not_s_mpdu_beacons, 50);
    assert_eq!(report.rx_ampdu.len(), 1);
    assert_eq!(report.rx_ampdu[0].protocol_ampdu_datagrams, 5_000);
    assert_eq!(report.rx_ampdu[0].unavailable_datagrams, 0);
}

#[test]
fn qualifies_complete_he20_evidence() {
    let report = parse_device_report(
        "\0OTX b=50000000 d=1 u=5000000 k=80000 e=0 w=20 r=114700 g=1 x=0 l=1 a=1342257664\n\
             OAMP aggregates=120 publications=121 completed=120 subframes=3744 acknowledged=3744 single=2 individual_retry=0 timeout=0 collision=0 min=2 max=32 stop_frame=116 stop_capacity=0 stop_empty=4\n\
             OAMPH one=0 two_three=1 four_seven=1 eight_fifteen=1 sixteen_twentythree=1 twentyfour_thirty=0 thirtyone=0 full32=116\n\
             OAMPT preparation_us=1200 preparation_max_us=14 publication_us=605 publication_max_us=8 exchange_us=24000 exchange_max_us=240 first_exchanges=119 first_exchange_us=23760 first_exchange_max_us=210 retried_exchanges=1 retry_publications=2 retry_exchange_us=240 retry_exchange_max_us=240\n\
             OAMPI tx_irq_epochs=121 tx_irq_samples=2 tx_irq_skew=0 tx_irq_service_us=18 tx_irq_service_max_us=11 tx_flight_samples=2 tx_flight_us=390 tx_flight_max_us=210\n\
             OAMPB samples=121 received=120 success_without=0 nonzero_control=0 start_outside=0 start_lag_max=31 full=120 partial=0 empty=1\n\
             ORX b=6000000 d=5000 u=5000000 k=9600 code=1343154946\n\
             ORXP f=4 r=11 m=11\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=0 forward_missing=0 maximum_gap=0 maximum_gap_at=4294967295 first_gap_at=4294967295 last_gap_at=4294967295 backward=0 adjacent_duplicates=0 unsequenced=0 maximum_interarrival_us=100 maximum_interarrival_at=1\n\
             ORXSM s_mpdu=100 not_s_mpdu=4900 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXSC samples=5000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63\n\
             ORXD frames=5000 data=5000 waits=5000 wait_us=1000 wait_boot_max_us=2 dispatch_us=150000 dispatch_boot_max_us=35 publications=5000 bytes=7570000 publish_us=60000 publish_boot_max_us=15\n\
             ORXR starts=0 stops=0 start_tid=0 start_seq=6 window=8 first_samples=1 first_tid=0 first_start=6 first_seq=6 first_distance=0 buffered=3 released=5000 missing=0 stale=0 expiries=0 occupied=0 occupied_max=7\n\
             ORXF zero=0 one=5000 two_three=0 four_seven=0 eight_fifteen=0 sixteen_thirty_one=0 thirty_two_plus=0 irq_posts=5000 irq_epochs=5000 irq_entries=5000 irq_coalesced=0 irq_samples=5000 irq_skew=0 irq_service_us=25000 irq_service_boot_max_us=8\n\
             ORXI spurious=0 rx_only=5000 rx_mixed=0 tx_only=0 tx_mixed=0 other_only=0 extra=0 saturated=0 aux_entries=5000 unhandled_entries=0\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=protocol polls=5000 poll_us=180000 poll_boot_max_us=120 over_100us=1 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_rx polls=5000 poll_us=125000 poll_boot_max_us=90 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_tx polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=tcp polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_format=4\n",
    );
    let options = Config::default().validate().unwrap();
    let host = HostTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 6_250_000,
        datagrams: 5_000,
        elapsed: Duration::from_secs(5),
        maximum_lateness: Duration::from_micros(20),
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    qualify::qualify(&options, host, &report).unwrap();
    assert!(report.code_addresses.contains(&1_343_154_946));
    let ampdu = AmpduEvidence::from_report(&report);
    assert_eq!(ampdu.aggregates, 120);
    assert_eq!(ampdu.histogram_total(), 120);
    assert_eq!(ampdu.full32, 116);
    assert_eq!(ampdu.maximum, 32);
    assert_eq!(ampdu.preparation_us, 1_200);
    assert_eq!(ampdu.first_exchanges, 119);
    assert_eq!(ampdu.first_exchange_us, 23_760);
    assert_eq!(ampdu.retried_exchanges, 1);
    assert_eq!(ampdu.retry_publications, 2);
    assert_eq!(ampdu.retry_exchange_us, 240);
    assert_eq!(ampdu.tx_irq_samples, 2);
    assert_eq!(ampdu.tx_irq_service_us, 18);
    assert_eq!(ampdu.tx_flight_samples, 2);
    assert_eq!(ampdu.tx_flight_us, 390);
    assert_eq!(ampdu.tx_flight_max_us, 210);
}

#[test]
fn parses_current_production_rx_benchmark_evidence() {
    let mut report = parse_device_report(
        "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes=6000000 datagrams=5000 elapsed_us=5000000 throughput_kbps=9600 receive_errors=0 terminal=1\n\
             OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path mpdu=5000 data_success=5000 fcs_error=0 buffer_full=0 fifo_overflow=0 enqueued=5000 queue_dropped=0 rx_irqs=5000 reload_delays=0 rx_format=4\n\
             ORXQ first=0 highest=4999 next=5000 gap_events=2 forward_missing=4 maximum_gap=3 maximum_gap_at=100 first_gap_at=100 last_gap_at=4000 backward=1 adjacent_duplicates=2 unsequenced=0 maximum_interarrival_us=300 maximum_interarrival_at=4000\n\
             ORXSM s_mpdu=50 not_s_mpdu=4950 unavailable=0 beacon_s_mpdu=0 beacon_not_s_mpdu=50 beacon_unavailable=0\n\
             ORXAG ampdu=5000 not_ampdu=0 hardware_ampdu=0 hardware_not_ampdu=0 protocol_ampdu=5000 protocol_not_ampdu=0 unavailable=0\n\
             ORXM m0=0 m1=0 m2=0 m3=0 m4=0 m5=0 m6=0 m7=20 m8=30 m9=4950 m10=0 m11=0 other=0\n\
             ORXHTL m0=0 m1=0 m2=0 m3=0 m4=0 m5=0 m6=0 m7=5000\n\
             ORXHTS m0=0 m1=0 m2=0 m3=0 m4=0 m5=0 m6=0 m7=0\n\
             ORXHTW m0=0 m1=0 m2=0 m3=0 m4=0 m5=0 m6=0 m7=0 other=0\n\
             ORXS calls=5000 frontier=5000 admitted=5000 bytes=7800000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63 fmax=1 amax=1 service_us=100000 service_boot_max_us=24\n\
             ORXSC samples=5000 back=0 bulk_blocked=0 pool=0 queue=0 deferred_max=0 pool_min=4294967295 queue_min=4294967295 pool_floor=31 queue_floor=63\n\
             ORXB increments=2 samples=1 between=1 during=1 between_samples=1 during_samples=1 last_service=6123 last_phase=3 last_counter=17 last_frontier=7 last_admitted=7 last_pool=60 last_queue=61 last_service_us=73\n\
             ORXD frames=5000 data=5000 waits=5000 wait_us=1000 wait_boot_max_us=2 dispatch_us=150000 dispatch_boot_max_us=35 publications=5000 bytes=7570000 publish_us=60000 publish_boot_max_us=15\n\
             ORXR starts=0 stops=0 start_tid=0 start_seq=6 window=8 first_samples=1 first_tid=0 first_start=6 first_seq=6 first_distance=0 buffered=3 released=5000 missing=0 stale=0 expiries=0 occupied=0 occupied_max=7\n\
             ORXF zero=0 one=5000 two_three=0 four_seven=0 eight_fifteen=0 sixteen_thirty_one=0 thirty_two_plus=0 irq_posts=5000 irq_epochs=5000 irq_entries=5000 irq_coalesced=0 irq_samples=5000 irq_skew=0 irq_service_us=25000 irq_service_boot_max_us=8\n\
             ORXI spurious=0 rx_only=5000 rx_mixed=0 tx_only=0 tx_mixed=0 other_only=0 extra=0 saturated=0 aux_entries=5000 unhandled_entries=0\n\
             ORTP task=network polls=5100 poll_us=210000 poll_boot_max_us=140 over_100us=2 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=protocol polls=5000 poll_us=180000 poll_boot_max_us=120 over_100us=1 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=radio polls=5000 poll_us=390000 poll_boot_max_us=310 over_100us=20 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_rx polls=5000 poll_us=125000 poll_boot_max_us=90 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=udp_tx polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             ORTP task=tcp polls=0 poll_us=0 poll_boot_max_us=0 over_100us=0 over_500us=0 over_1000us=0 over_5000us=0\n\
             OTX b=50000000 d=1 u=5000000 k=80000 e=0 w=20 r=114700 g=1 x=0 l=1 a=1342257664\n\
             OAMP aggregates=120 publications=120 completed=120 subframes=3840 acknowledged=3840 single=0 individual_retry=0 timeout=0 collision=0 min=32 max=32 stop_frame=120 stop_capacity=0 stop_empty=0\n\
             OAMPH one=0 two_three=0 four_seven=0 eight_fifteen=0 sixteen_twentythree=0 twentyfour_thirty=0 thirtyone=0 full32=120\n\
             OAMPT preparation_us=1200 preparation_max_us=12 publication_us=600 publication_max_us=6 exchange_us=24000 exchange_max_us=210 first_exchanges=120 first_exchange_us=24000 first_exchange_max_us=210 retried_exchanges=0 retry_publications=0 retry_exchange_us=0 retry_exchange_max_us=0\n\
             OAMPI tx_irq_epochs=120 tx_irq_samples=2 tx_irq_skew=0 tx_irq_service_us=17 tx_irq_service_max_us=10 tx_flight_samples=2 tx_flight_us=388 tx_flight_max_us=204\n\
             OAMPB samples=120 received=120 success_without=0 nonzero_control=0 start_outside=0 start_lag_max=31 full=120 partial=0 empty=0\n",
    );

    assert_eq!(report.rx.len(), 1);
    assert_eq!(report.rx[0].throughput_kbps, 9_600);
    assert_eq!(report.rx_formats, [4]);
    assert_eq!(report.dma_health, [(0, 0)]);
    assert_eq!(report.software_health, [(5_000, 0)]);
    assert_eq!(
        report.rx_mcs_histograms,
        [([0, 0, 0, 0, 0, 0, 0, 20, 30, 4_950, 0, 0], 0)]
    );
    assert_eq!(report.rx_ht40_long_gi_histograms[0][7], 5_000);
    let rx = qualify_rx_report(&report, 4).unwrap();
    assert_eq!(rx.he_mcs_histogram[9], 4_950);
    assert_eq!(rx.s_mpdu.not_s_mpdu_datagrams, 4_950);
    assert_eq!(rx.s_mpdu.s_mpdu_datagrams, 50);
    assert_eq!(rx.s_mpdu.not_s_mpdu_beacons, 50);
    assert_eq!(rx.ampdu.protocol_ampdu_datagrams, 5_000);
    assert_eq!(rx.ampdu.unavailable_datagrams, 0);
    assert_eq!(rx.pipeline.service_calls, 5_000);
    assert_eq!(rx.pipeline.service_us, 100_000);
    assert_eq!(rx.pipeline.dma_buffer_full_increments, 2);
    assert_eq!(rx.pipeline.dma_buffer_full_service_samples, 1);
    assert_eq!(rx.pipeline.dma_buffer_full_between_services, 1);
    assert_eq!(rx.pipeline.dma_buffer_full_during_services, 1);
    assert_eq!(rx.pipeline.dma_buffer_full_between_service_samples, 1);
    assert_eq!(rx.pipeline.dma_buffer_full_during_service_samples, 1);
    assert_eq!(rx.pipeline.dma_buffer_full_last_service, 6_123);
    assert_eq!(rx.pipeline.dma_buffer_full_last_phase, 3);
    assert_eq!(rx.pipeline.dma_buffer_full_last_counter, 17);
    assert_eq!(rx.pipeline.dma_buffer_full_last_frontier, 7);
    assert_eq!(rx.pipeline.dma_buffer_full_last_admitted, 7);
    assert_eq!(rx.pipeline.dma_buffer_full_last_pool_credits, 60);
    assert_eq!(rx.pipeline.dma_buffer_full_last_queue_credits, 61);
    assert_eq!(rx.pipeline.dma_buffer_full_last_service_us, 73);
    assert_eq!(rx.pipeline.network_publish_us, 60_000);
    assert_eq!(rx.pipeline.frontier_one_services, 5_000);
    assert_eq!(rx.pipeline.rx_irq_to_service_us, 25_000);
    assert_eq!(rx.irq.rx_only_entries, 5_000);
    assert_eq!(rx.irq.classified_entries(), 5_000);
    assert_eq!(rx.irq.auxiliary_entries, 5_000);
    assert_eq!(rx.irq.unhandled_entries, 0);
    assert_eq!(rx.task_polls.network.polls, 5_100);
    assert_eq!(rx.task_polls.radio.poll_us, 390_000);
    assert_eq!(rx.task_polls.radio.over_100us, 20);
    assert_eq!(rx.sequence.first, Some(0));
    assert_eq!(rx.sequence.highest, Some(4_999));
    assert_eq!(rx.sequence.forward_missing, 4);
    assert_eq!(rx.sequence.maximum_gap_at, Some(100));
    assert_eq!(rx.sequence.backward, 1);
    assert_eq!(rx.sequence.adjacent_duplicates, 2);
    assert_eq!(rx.sequence.maximum_interarrival_us, 300);
    assert_eq!(rx.sequence.maximum_interarrival_at, Some(4_000));
    assert_eq!(rx.reorder.window, 8);
    assert_eq!(rx.reorder.first_start_sequence, 6);
    assert_eq!(rx.reorder.first_frame_sequence, 6);
    assert_eq!(rx.reorder.buffered, 3);
    assert_eq!(rx.reorder.released, 5_000);
    assert_eq!(rx.reorder.maximum_occupied, 7);
    report.rx_reorder[0].first_frame_sequence = 28;
    report.rx_reorder[0].first_distance = 22;
    assert!(qualify_rx_report(&report, 4).is_ok());
    report.rx_reorder[0].first_frame_sequence = 5;
    report.rx_reorder[0].first_distance = 0x0fff;
    assert!(
        qualify_rx_report(&report, 4)
            .unwrap_err()
            .to_string()
            .contains("invalid first RX reorder frame")
    );
    report.rx_reorder[0].first_frame_sequence = 6;
    report.rx_reorder[0].first_distance = 0;
    assert_eq!(report.ampdu.len(), 1);
    assert_eq!(report.ampdu_histograms[0].full32, 120);
    assert_eq!(report.ampdu_timings[0].exchange_max_us, 210);

    report.rx_formats[0] = 2;
    report.rx_ampdu[0] = RxAmpduEvidence {
        ampdu_datagrams: 4_500,
        not_ampdu_datagrams: 500,
        hardware_ampdu_datagrams: 4_500,
        hardware_not_ampdu_datagrams: 498,
        protocol_not_ampdu_datagrams: 2,
        ..RxAmpduEvidence::default()
    };
    assert_eq!(
        qualify_rx_report(&report, 2).unwrap().ampdu.ampdu_datagrams,
        4_500
    );
    report.rx_ampdu[0].protocol_ampdu_datagrams = 1;
    report.rx_ampdu[0].hardware_ampdu_datagrams = 4_499;
    assert!(
        qualify_rx_report(&report, 2)
            .unwrap_err()
            .to_string()
            .contains("did not remain hardware-sourced")
    );
    report.rx_ampdu[0].protocol_ampdu_datagrams = 0;
    report.rx_ampdu[0].hardware_ampdu_datagrams = 4_500;
    report.rx_ampdu[0].unavailable_datagrams = 1;
    assert!(
        qualify_rx_report(&report, 2)
            .unwrap_err()
            .to_string()
            .contains("A-MPDU provenance unavailable")
    );
    report.rx_ampdu[0] = RxAmpduEvidence {
        ampdu_datagrams: 0,
        not_ampdu_datagrams: 5_000,
        hardware_not_ampdu_datagrams: 5_000,
        ..RxAmpduEvidence::default()
    };
    assert!(
        qualify_rx_report(&report, 2)
            .unwrap_err()
            .to_string()
            .contains("did not observe an aggregated benchmark MPDU")
    );
    report.rx_formats[0] = 4;
    report.rx_ampdu[0] = RxAmpduEvidence {
        ampdu_datagrams: 5_000,
        not_ampdu_datagrams: 0,
        protocol_ampdu_datagrams: 5_000,
        ..RxAmpduEvidence::default()
    };

    report.rx_s_mpdu[0].unavailable_datagrams = 1;
    assert!(
        qualify_rx_report(&report, 4)
            .unwrap_err()
            .to_string()
            .contains("S-MPDU provenance unavailable")
    );
    report.rx_s_mpdu[0].unavailable_datagrams = 0;
    report.rx_reorder[0].maximum_occupied = 8;
    assert!(
        qualify_rx_report(&report, 4)
            .unwrap_err()
            .to_string()
            .contains("invalid RX reorder occupancy")
    );
    report.rx_reorder[0].maximum_occupied = 7;
    report.dma_health[0] = (2, 0);
    let assessment = assess_rx_report(&report, 4).unwrap();
    assert_eq!(assessment.rx.buffer_full, 2);
    assert_eq!(assessment.rx.sequence.forward_missing, 4);
    assert_eq!(
        assessment.failure.as_deref(),
        Some("RX DMA starvation: buffer_full=2 fifo_overflow=0")
    );
}

#[test]
fn offered_rate_only_supplies_a_missing_tx_floor() {
    let implicit = Config {
        tx_rate_bps: Some(50_000_000),
        ..Default::default()
    }
    .validate()
    .unwrap();
    assert_eq!(implicit.tx_floor_bps, Some(45_000_000));
    let explicit = Config {
        tx_rate_bps: Some(50_000_000),
        tx_floor_bps: Some(40_000_000),
        ..Default::default()
    }
    .validate()
    .unwrap();
    assert_eq!(explicit.tx_floor_bps, Some(40_000_000));
}

#[test]
fn combined_floor_requires_sufficient_offered_traffic() {
    assert!(
        Config {
            rate_bps: 10_000_000,
            tx_rate_bps: Some(20_000_000),
            combined_floor_bps: Some(31_000_000),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            combined_floor_bps: Some(10_000_000),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
