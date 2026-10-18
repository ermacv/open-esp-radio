use super::*;

/// Criteria that accept any per-flow bitrate.
fn criteria() -> MultiClientCriteria {
    MultiClientCriteria {
        exact_delivery: false,
        minimum_rx_bps: None,
        minimum_tx_bps: None,
        minimum_combined_bps: None,
        minimum_bps_per_flow: 0,
        maximum_flow_skew_percent: None,
        minimum_host_offer_percent: None,
        maximum_secondary_tx_interarrival_ms: None,
        minimum_secondary_tx_datagrams: None,
    }
}

fn observation(target: Result<MultiClientTarget>) -> MultiClientObservation {
    MultiClientObservation {
        direction: Direction::Tx,
        duration: Duration::from_secs(12),
        peers: [
            Ipv4Endpoint {
                address: [10, 43, 0, 2],
                port: 9002,
            },
            Ipv4Endpoint {
                address: [10, 43, 0, 3],
                port: 9003,
            },
        ],
        host_tx: [None, None],
        host_requested_bps: [None, None],
        host_rx: [
            Some(vec![Burst {
                bytes: 1472,
                datagrams: 1,
                started_at_zero: true,
                ..Burst::default()
            }]),
            Some(vec![]),
        ],
        target,
        host_errors: Vec::new(),
    }
}

#[test]
fn missing_terminal_evidence_preserves_each_hosts_delivery() {
    let output = tempfile::tempdir().unwrap();
    let results = oer_hil_workload::results::Results::default();
    let error = observation(Err("terminal evidence timed out".into()))
        .evaluate(
            output.path(),
            &criteria(),
            &oer_hil_workload::measurements::Recorder::default(),
            &results,
        )
        .err()
        .expect("qualification must fail");
    assert!(error.to_string().contains("terminal evidence timed out"));
    let saved: serde_json::Value = results.snapshot().observations[0].value.clone();
    assert_eq!(saved["host_rx"][0][0]["bytes"], 1472);
    assert_eq!(saved["host_rx"][1], serde_json::json!([]));
    assert!(saved["target_flows"].is_null());
    assert_eq!(saved["duration_micros"], 12_000_000);
}

#[test]
fn failed_per_peer_rate_gate_preserves_complete_raw_evidence() {
    let output = tempfile::tempdir().unwrap();
    let results = oer_hil_workload::results::Results::default();
    let flows = std::array::from_fn(|index| {
        Some(FlowTransportEvidence {
            rx_maximum_silence_micros: None,
            flow_id: index as u8,
            rx_bytes: 0,
            rx_units: 0,
            tx_bytes: 1472,
            tx_units: 1,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: 12_000_000,
            transport_errors: 0,
        })
    });
    let mut observed = observation(Ok(MultiClientTarget {
        elapsed_micros: 12_000_000,
        flows,
    }));
    observed.host_rx[1] = observed.host_rx[0].clone();
    let criteria = MultiClientCriteria {
        exact_delivery: false,
        minimum_bps_per_flow: 10_000,
        ..criteria()
    };
    let error = observed
        .evaluate(
            output.path(),
            &criteria,
            &oer_hil_workload::measurements::Recorder::default(),
            &results,
        )
        .err()
        .expect("qualification must fail");
    assert!(error.to_string().contains("below required 10000"));
    let saved: serde_json::Value = results.snapshot().observations[0].value.clone();
    assert_eq!(saved["target_flows"][1]["tx_bytes"], 1472);
    assert_eq!(saved["host_rx"][1][0]["bytes"], 1472);
    assert!(saved["target_error"].is_null());
}

#[test]
fn socket_setup_does_not_prime_an_unready_target() {
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let std::net::SocketAddr::V4(peer_address) = peer.local_addr().unwrap() else {
        panic!("IPv4 peer expected")
    };
    let socket = open_multi_client_socket(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
        Some(peer_address),
    )
    .unwrap();
    let mut packet = [0_u8; 16];
    let error = peer.recv_from(&mut packet).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    socket.send(&[0]).unwrap();
    assert_eq!(peer.recv_from(&mut packet).unwrap().0, 1);
}

#[test]
fn failed_sender_keeps_successful_receiver_observations() {
    let output = tempfile::tempdir().unwrap();
    let results = oer_hil_workload::results::Results::default();
    let mut observed = observation(Ok(MultiClientTarget {
        elapsed_micros: 12_000_000,
        flows: [None, None],
    }));
    observed.host_errors.push("flow 1 sender failed".to_owned());
    assert!(
        observed
            .evaluate(
                output.path(),
                &criteria(),
                &oer_hil_workload::measurements::Recorder::default(),
                &results,
            )
            .is_err()
    );
    let saved: serde_json::Value = results.snapshot().observations[0].value.clone();
    assert_eq!(saved["host_errors"][0], "flow 1 sender failed");
    assert_eq!(saved["host_rx"][0][0]["bytes"], 1472);
}

#[test]
fn unmet_offer_preserves_delivery_and_reports_the_invalid_load_condition() {
    let output = tempfile::tempdir().unwrap();
    let results = oer_hil_workload::results::Results::default();
    let mut observed = observation(Err("terminal timeout".into()));
    observed.direction = Direction::Bidirectional;
    observed.host_requested_bps = [Some(1_000_000), Some(1_000_000)];
    observed.host_tx = [Some(UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 750_000,
        datagrams: 750,
        elapsed: Duration::from_secs(12),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    }); 2];
    let error = observed
        .evaluate(
            output.path(),
            &MultiClientCriteria {
                minimum_host_offer_percent: Some(95),
                ..criteria()
            },
            &oer_hil_workload::measurements::Recorder::default(),
            &results,
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("offered load not met"));
    let saved: serde_json::Value = results.snapshot().observations[0].value.clone();
    assert_eq!(saved["host_offer"][0]["status"], "under-offered");
    assert_eq!(saved["host_offer"][1]["accepted_bps_over_window"], 500_000);
    assert_eq!(saved["host_rx"][0][0]["bytes"], 1472);
    assert_eq!(saved["target_error"], "terminal timeout");
}
