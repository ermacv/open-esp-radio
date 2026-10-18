use super::*;

#[test]
fn parses_core0_tx_cycles_and_instructions() {
    let evidence = Core0CoarseEvidence::from_log(
        "ORC0C rx_irq_posts=154 radio_polls=63030 radio_cycles=2950392831 radio_instret=593043026 poll_to_runner_cycles=21183800",
    )
    .unwrap();
    assert_eq!(evidence.radio_polls, 63_030);
    assert_eq!(evidence.radio_cycles, 2_950_392_831);
    assert_eq!(evidence.radio_instructions, 593_043_026);
    // The typed result names the raw counters; occupancy and IPC derive
    // from them in analysis.
    let value = serde_json::to_value(evidence).unwrap();
    assert_eq!(value["radio_cycles"], 2_950_392_831_u64);
    assert_eq!(value["radio_instructions"], 593_043_026_u64);
}

#[test]
fn parses_non_overlapping_tx_phase_counters() {
    let evidence = TxPhaseEvidence::from_log(
        "ORC0TX start_calls=2 start_cycles=20 start_instret=10 prepare_calls=3 prepare_cycles=30 prepare_instret=15 publish_calls=4 publish_cycles=40 publish_instret=20 service_calls=5 service_cycles=50 service_instret=25\n\
         ORC0TXN encode_calls=10 encode_cycles=70 encode_instret=35 commit_calls=10 commit_cycles=80 commit_instret=40\n\
         ONTX admission_attempts=12 admission_successes=10 admission_cycles=120 admission_instret=60 consume_calls=10 consume_bytes=14720 consume_cycles=500 consume_instret=250 emit_cycles=300 emit_instret=150 publication_cycles=200 publication_instret=100",
    )
    .unwrap();
    assert_eq!(evidence.core0_prepare_cycles, 30);
    assert_eq!(evidence.core0_encode_cycles, 70);
    assert_eq!(evidence.core1_admission_attempts, 12);
    assert_eq!(evidence.core1_admission_successes, 10);
    assert_eq!(evidence.core1_publication_cycles, 200);
    let value = serde_json::to_value(evidence).unwrap();
    assert_eq!(value["core0_service_cycles"], 50);
    assert_eq!(value["core1_admission_attempts"], 12);
}

#[test]
fn burst_distinguishes_reordering_from_unrecovered_loss() {
    let now = Instant::now();
    let mut burst = ActiveBurst::new(0, 100, now);
    burst.push(1, 100, now);
    burst.push(3, 100, now);
    burst.push(2, 100, now);
    let evidence = burst.finish();
    assert_eq!(evidence.missing, 0);
    assert_eq!(evidence.reordered, 1);
    assert_eq!(evidence.duplicates, 0);
}

#[test]
fn burst_reports_unrecovered_loss_and_duplicates_separately() {
    let now = Instant::now();
    let mut burst = ActiveBurst::new(0, 100, now);
    burst.push(2, 100, now);
    burst.push(2, 100, now);
    let evidence = burst.finish();
    assert_eq!(evidence.missing, 1);
    assert_eq!(evidence.reordered, 0);
    assert_eq!(evidence.duplicates, 1);
    assert_eq!(evidence.missing_runs, 1);
    assert_eq!(evidence.maximum_missing_run, 1);
    assert_eq!(evidence.maximum_missing_run_start, Some(1));
    assert_eq!(evidence.maximum_missing_run_end, Some(1));
}

#[test]
fn burst_reports_contiguous_missing_sequence_runs() {
    let now = Instant::now();
    let mut burst = ActiveBurst::new(0, 100, now);
    burst.push(3, 100, now);
    burst.push(4, 100, now);
    burst.push(8, 100, now);
    let evidence = burst.finish();
    assert_eq!(evidence.missing, 5);
    assert_eq!(evidence.missing_runs, 2);
    assert_eq!(evidence.maximum_missing_run, 3);
    assert_eq!(evidence.maximum_missing_run_start, Some(5));
    assert_eq!(evidence.maximum_missing_run_end, Some(7));
}

#[test]
fn burst_records_sequence_range_and_largest_interarrival() {
    let now = Instant::now();
    let mut burst = ActiveBurst::new(10, 100, now);
    burst.push(11, 100, now + Duration::from_micros(25));
    burst.push(12, 100, now + Duration::from_micros(125));
    let evidence = burst.finish();
    assert_eq!(evidence.lowest_sequence, 10);
    assert_eq!(evidence.highest_sequence, 12);
    assert_eq!(evidence.maximum_interarrival_us, 100);
    assert_eq!(evidence.sequence_after_maximum_interarrival, Some(12));
}

#[test]
fn incomplete_burst_summary_distinguishes_missing_sequence_zero_from_no_traffic() {
    let bursts = [Burst {
        datagrams: 17,
        lowest_sequence: 41,
        highest_sequence: 57,
        ..Burst::default()
    }];
    assert_eq!(
        describe_bursts(&bursts),
        "observed_bursts=1 observed_datagrams=17 zero_started=0 sequence_range=Some(41)..=Some(57)"
    );
    assert_eq!(
        describe_bursts(&[]),
        "observed_bursts=0 observed_datagrams=0 zero_started=0 sequence_range=None..=None"
    );
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            duration: Duration::from_secs(7),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            maximum_idle_channel_utilization_255: Some(0),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn short_udp_datagram_does_not_reuse_a_previous_packets_sequence() {
    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    sender
        .send_to(&0_u32.to_be_bytes(), receiver.local_addr().unwrap())
        .unwrap();
    sender
        .send_to(&[0], receiver.local_addr().unwrap())
        .unwrap();
    let output = tempfile::tempdir().unwrap();
    let bursts = Receiver::start(
        &receiver,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(1),
        output.path(),
        "short",
    )
    .unwrap()
    .finish(Some(1))
    .unwrap();
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0].datagrams, 1);
    assert_eq!(bursts[0].duplicates, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn connected_udp_error_is_not_silently_attributed_to_a_previous_epoch() {
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.connect(peer.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    drop(peer);
    socket.send(&[0]).unwrap();
    let output = tempfile::tempdir().unwrap();
    let error = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(1),
        output.path(),
        "error",
    )
    .unwrap()
    .finish(Some(1))
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::ConnectionRefused
    );
}
