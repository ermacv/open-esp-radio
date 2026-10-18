use super::*;

#[test]
fn retention_gate_reports_loss_before_radio_instead_of_a_generic_delivery_failure() {
    use oer_hil_protocol::{wifi::WifiAccessPointEvidence, wifi::WifiTxRetentionEvidence};
    let stopped = WifiAccessPointEvidence {
        tx_retention: Some(WifiTxRetentionEvidence {
            unicast_power_save_full: 3,
            ..Default::default()
        }),
        ..Default::default()
    };
    let error =
        validate_access_point_observation(2, WifiAccessPointSecurity::Wpa2Personal, 2, &stopped)
            .unwrap_err()
            .to_string();
    assert!(error.contains("TX retention dropped admitted frames"));
    assert!(error.contains("unicast_power_save_full: 3"));
}

#[test]
fn protocol_rejection_gate_names_the_exact_retained_failure() {
    use oer_hil_protocol::{
        wifi::WifiAccessPointEvidence, wifi::WifiRxRejection, wifi::WifiRxRejectionReason,
    };
    let evidence = WifiAccessPointEvidence {
        protected_data_protocol_rejected: 1,
        first_rx_protocol_rejection: Some(WifiRxRejection {
            reason: WifiRxRejectionReason::Replay {
                packet_number: 5,
                highest: 7,
            },
            at_micros: 1200,
            transmitter: Some([2; 6]),
            frame_control: Some(0x4188),
            sequence_control: Some(32),
            tid: Some(0),
            key_id: Some(0),
            packet_number: Some(5),
            mpdu_length: 100,
        }),
        ..Default::default()
    };
    let error =
        validate_access_point_observation(3, WifiAccessPointSecurity::Wpa2Personal, 2, &evidence)
            .unwrap_err()
            .to_string();
    assert!(error.contains("AP cycle 3: 1 RX protocol rejection"));
    assert!(error.contains("Replay { packet_number: 5, highest: 7 }"));
    assert!(error.contains("at_micros: 1200"));
}

#[test]
fn cleanup_annotation_preserves_fixture_cancellation_and_scenario_causes() {
    for (primary, expected, cancelled) in [
        (
            Box::new(oer_hil_family_ieee80211_fixture::Error::new(
                "SSH transport failed",
            )) as Box<dyn std::error::Error + Send + Sync>,
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure,
            false,
        ),
        (
            oer_hil_family_ieee80211_fixture::Error::context(Box::new(oer_process::Cancelled)),
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure,
            true,
        ),
        (
            "secondary AP client lost traffic".into(),
            oer_hil_run_bundle_format::run::FailureKind::Scenario,
            false,
        ),
    ] {
        let error = with_cleanup_errors(primary, Some("restore failed".into()), None, None, None);
        assert_eq!(oer_hil_workload::failure::classify(&*error).kind, expected);
        assert_eq!(oer_process::is_cancelled(&*error), cancelled);
        assert!(error.to_string().contains("client restore failed"));
    }
}
use oer_hil_protocol::{
    network::Finished, network::RadioEvidence, network::ResultSummary,
    network::RxConsumerLedgerEvidence, network::RxDeliveryEvidence, network::RxRadioEvidence,
    network::RxSequenceStageEvidence, network::TransportEvidence, system::StackUsage,
    system::StackWatermark,
};

fn evidence(rx_bytes: u64, tx_bytes: u64, rx_units: u64, tx_units: u64) -> SessionEvidence {
    let transport = TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes,
        tx_bytes,
        rx_units,
        tx_units,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 1,
        transport_errors: 0,
    };
    SessionEvidence {
        link: oer_hil_protocol::base::LinkHealth {
            rx_frames: 1,
            rx_cobs_errors: 0,
            rx_checksum_errors: 0,
            rx_decode_errors: 0,
            rx_overflows: 0,
            tx_frames: 1,
            tx_dropped: 0,
            text_dropped: 0,
            text_truncated: 0,
        },
        finished: Finished {
            summary: ResultSummary {
                verdict: oer_hil_protocol::network::SessionVerdict::Passed,
                evidence_records: 0,
            },
            evidence_crc32c: 0,
        },
        transport,
        flow_transport: [
            Some(
                oer_hil_protocol::network::FlowTransportEvidence::from_session_total(0, transport),
            ),
            None,
        ],
        radio: (rx_units != 0).then_some(RadioEvidence {
            rx: Some(RxRadioEvidence {
                sequence_first: Some(0),
                sequence_highest: u32::try_from(rx_units)
                    .ok()
                    .and_then(|units| units.checked_sub(1)),
                ..RxRadioEvidence::default()
            }),
            tx: None,
        }),
        tx_timing: None,
        rx_delivery: None,
        network_scheduler: None,
        rx_zero_copy: None,
        cpu_load: None,
        stack: StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
            cpu0: StackWatermark {
                capacity_bytes: 1,
                free_bytes: 1,
                used_bytes: 0,
                minimum_free_bytes: 1,
            },
            cpu1: StackWatermark {
                capacity_bytes: 1,
                free_bytes: 1,
                used_bytes: 0,
                minimum_free_bytes: 1,
            },
        },
    }
}

#[test]
fn udp_exact_delivery_checks_both_directions() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    assert!(
        validate_udp(
            Some(sent),
            Some(&[received]),
            evidence(1_200, 1_200, 1, 1),
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_ok()
    );
}

#[test]
fn udp_ordering_defect_fails_closed() {
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        missing: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    assert!(
        validate_udp(
            None,
            Some(&[received]),
            evidence(0, 1_200, 0, 1),
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_err()
    );
}

#[test]
fn udp_target_rx_reordering_fails_closed_even_when_counts_match() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 2_400,
        datagrams: 2,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut reordered = evidence(2_400, 0, 2, 0);
    reordered
        .radio
        .as_mut()
        .and_then(|radio| radio.rx.as_mut())
        .expect("RX evidence")
        .sequence_backward = 1;

    assert!(
        validate_udp(
            Some(sent),
            None,
            reordered,
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_err()
    );
}

#[test]
fn udp_characterization_reports_receiver_delivery_and_allows_loss() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 2_400,
        datagrams: 2,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        missing: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    let delivered = validate_udp(
        Some(sent),
        Some(&[received]),
        evidence(1_200, 2_400, 1, 2),
        UdpEvidencePolicy {
            exact_delivery: false,
            driver_observation: true,
            rx_delivery: false,
        },
    )
    .unwrap();

    assert_eq!(delivered, Some(received));
}

#[test]
fn observer_free_udp_rx_accepts_transport_without_driver_evidence() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut observed = evidence(1_200, 0, 1, 0);
    observed.radio = None;

    assert!(
        validate_udp(
            Some(sent),
            None,
            observed,
            UdpEvidencePolicy {
                exact_delivery: false,
                driver_observation: false,
                rx_delivery: false,
            },
        )
        .is_ok()
    );
}

#[test]
fn rx_delivery_diagnostic_fails_closed_without_publication_evidence() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };

    let error = validate_udp(
        Some(sent),
        None,
        evidence(1_200, 0, 1, 0),
        UdpEvidencePolicy {
            exact_delivery: true,
            driver_observation: true,
            rx_delivery: true,
        },
    )
    .expect_err("diagnostic evidence must be mandatory");

    assert!(error.to_string().contains("typed RX delivery evidence"));
}

#[test]
fn rx_delivery_diagnostic_rejects_evidence_from_the_wrong_publication_edge() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut observed = evidence(1_200, 0, 1, 0);
    observed.rx_delivery = Some(RxDeliveryEvidence {
        udp_consumer: RxSequenceStageEvidence {
            data_units: 1,
            first: Some(0),
            highest: Some(0),
            ..Default::default()
        },
        consumer_ledger: RxConsumerLedgerEvidence {
            unexpected_consumer: 1,
            first_observed: Some(0),
            ..Default::default()
        },
        ..Default::default()
    });

    let error = validate_udp(
        Some(sent),
        None,
        observed,
        UdpEvidencePolicy {
            exact_delivery: true,
            driver_observation: true,
            rx_delivery: true,
        },
    )
    .expect_err("misplaced diagnostic evidence must fail closed");

    assert!(error.to_string().contains("typed AP RX delivery frontier"));
}

#[test]
fn tcp_exact_delivery_checks_both_directions() {
    let sent = TcpTransmission {
        bytes: 8_192,
        writes: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_writes: 1,
        deadline_resets: 0,
    };
    let received = TcpReception {
        bytes: 8_192,
        reads: 1,
        elapsed: Duration::from_secs(1),
        pattern_errors: 0,
        eof: true,
    };
    assert!(validate_tcp(Some(sent), Some(received), evidence(8_192, 8_192, 1, 1)).is_ok());
}

#[test]
fn ap_rate_gate_checks_combined_bidirectional_throughput() {
    let report = SessionReport {
        direction: Direction::Bidirectional,
        rx_bytes: 2_000_000,
        tx_bytes: 2_000_000,
        rx_units: 0,
        tx_units: 0,
        elapsed_micros: 1_000_000,
    };
    let mut criteria = RateFloors {
        minimum_combined_bps: Some(32_000_000),
        ..RateFloors::default()
    };
    let measurements = oer_hil_workload::measurements::Recorder::default();
    assert!(validate_rate_criteria(&report, criteria, &measurements).is_ok());
    criteria.minimum_combined_bps = Some(32_000_001);
    assert!(validate_rate_criteria(&report, criteria, &measurements).is_err());
    // The verdict is recorded beside the aggregate rates it checked.
    let recorded = measurements.snapshot();
    let combined = recorded
        .iter()
        .find(|measurement| measurement.name == "ap.combined-rate")
        .unwrap();
    assert_eq!(combined.value, 32_000_000);
    assert_eq!(
        combined.threshold.map(|threshold| threshold.value),
        Some(32_000_001)
    );
    assert!(
        recorded
            .iter()
            .any(|m| m.name == "ap.rx.aggregate-rate" && m.value == 16_000_000)
    );
    assert!(
        recorded
            .iter()
            .any(|m| m.name == "ap.tx.aggregate-rate" && m.value == 16_000_000)
    );
}

#[test]
fn sparse_secondary_tx_requires_enough_packets_and_bounded_interarrival() {
    let flow = |flow_id, tx_units, maximum_interarrival_us| MultiClientFlowReport {
        flow_id,
        peer: Ipv4Endpoint {
            address: [10, 43, 0, flow_id.saturating_add(2)],
            port: 9_002 + u16::from(flow_id),
        },
        rx_bytes: 0,
        tx_bytes: tx_units * 1_472,
        rx_units: 0,
        tx_units,
        elapsed_micros: 22_000_000,
        rx_bps: 0,
        tx_bps: tx_units * 1_472 * 8_000_000 / 22_000_000,
        host_tx_started_at_zero: Some(true),
        host_tx_missing: Some(0),
        host_tx_reordered: Some(0),
        host_tx_duplicates: Some(0),
        host_tx_maximum_interarrival_us: Some(maximum_interarrival_us),
        host_tx_sequence_after_maximum_interarrival: Some(2),
    };
    let criteria = crate::scenario::access_point::MultiClientCriteria {
        exact_delivery: false,
        minimum_rx_bps: None,
        minimum_tx_bps: None,
        minimum_combined_bps: None,
        minimum_bps_per_flow: 0,
        maximum_flow_skew_percent: None,
        minimum_host_offer_percent: None,
        minimum_secondary_tx_datagrams: Some(8),
        maximum_secondary_tx_interarrival_ms: Some(5_500),
    };

    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 10, 5_003_241)],
            Direction::Tx,
            &criteria,
            &oer_hil_workload::measurements::Recorder::default(),
        )
        .is_ok()
    );
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 7, 5_003_241)],
            Direction::Tx,
            &criteria,
            &oer_hil_workload::measurements::Recorder::default(),
        )
        .unwrap_err()
        .to_string()
        .contains("delivered 7 datagrams")
    );
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 10, 5_500_001)],
            Direction::Tx,
            &criteria,
            &oer_hil_workload::measurements::Recorder::default(),
        )
        .unwrap_err()
        .to_string()
        .contains("inter-arrival")
    );
}

#[test]
fn ap_ht40_mcs7_gate_is_directional_and_fails_closed() {
    let link = Some(LinkExpectation {
        phy: PhyExpectation::Ht40,
        minimum_mcs: Some(7),
        guard_interval: oer_hil_scenario_catalog::link::HtGuardIntervalExpectation::Any,
        management_frame_protection: Default::default(),
        access_point_security: Default::default(),
    });
    let udp = |rx_bps, tx_bps| {
        AccessPointTraffic::Udp(crate::scenario::access_point::AccessPointUdp {
            duration_seconds: 1,
            payload_bytes: 1,
            offer: crate::scenario::Offer { rx_bps, tx_bps },
            criteria: Default::default(),
        })
    };
    let rx = udp(Some(1), None);
    let tx = udp(None, Some(1));
    let mut observed = oer_hil_protocol::wifi::WifiAccessPointEvidence::default();
    assert!(validate_mcs_evidence(&rx, link, &observed).is_err());
    observed.rx_ht_data_frames = 1;
    observed.rx_ht40_mcs_frames[7] = 1;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_ok());
    assert!(validate_mcs_evidence(&tx, link, &observed).is_err());
    observed.tx_ht_aggregates = 1;
    observed.tx_ht40_mcs7_aggregates = 1;
    assert!(validate_mcs_evidence(&tx, link, &observed).is_ok());
}

#[test]
fn ap_guard_interval_gate_tolerates_only_epoch_warmup_frames() {
    let link = Some(LinkExpectation {
        phy: PhyExpectation::Ht40,
        minimum_mcs: Some(7),
        guard_interval: HtGuardIntervalExpectation::Short,
        management_frame_protection: Default::default(),
        access_point_security: Default::default(),
    });
    let rx = AccessPointTraffic::Udp(crate::scenario::access_point::AccessPointUdp {
        duration_seconds: 1,
        payload_bytes: 1,
        offer: crate::scenario::Offer {
            rx_bps: Some(1),
            tx_bps: None,
        },
        criteria: Default::default(),
    });
    let mut observed = oer_hil_protocol::wifi::WifiAccessPointEvidence {
        rx_ht_data_frames: 100,
        rx_ht40_short_gi_frames: 99,
        rx_ht40_long_gi_frames: 1,
        ..Default::default()
    };
    observed.rx_ht40_mcs_frames[7] = 100;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_ok());

    observed.rx_ht40_short_gi_frames = 98;
    observed.rx_ht40_long_gi_frames = 2;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_err());
}

#[test]
fn ap_hardware_rx_health_uses_terminal_mac_counters() {
    let mut observed = oer_hil_protocol::wifi::WifiAccessPointEvidence::default();
    assert!(validate_rx_hardware_health(0, &observed).is_ok());

    observed.rx_hardware.buffer_full = 1;
    let error = validate_rx_hardware_health(2, &observed)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cycle 2"));
    assert!(error.contains("buffer_full=1"));

    observed.rx_hardware.buffer_full = 0;
    observed.rx_hardware.fifo_overflow = 1;
    assert!(validate_rx_hardware_health(3, &observed).is_err());
}

#[test]
fn functional_ap_observation_does_not_require_optional_pipeline_report() {
    let observed = oer_hil_protocol::wifi::WifiAccessPointEvidence {
        beacons_transmitted: 1,
        authentication_responses: 1,
        association_responses: 1,
        authorized_peers: 1,
        maximum_associated_peers: 1,
        maximum_authorized_peers: 1,
        peer_removals: 1,
        wpa2_response_windows: 2,
        disassociations_prepared: 1,
        disassociations_published: 1,
        disassociations_acknowledged: 1,
        deauthentications_prepared: 1,
        deauthentications_published: 1,
        deauthentications_acknowledged: 1,
        completed_rx_units: 0,
        completed_rx_descriptors: 0,
        recycled_rx_descriptors: 0,
        ..Default::default()
    };

    assert!(
        validate_access_point_observation(0, WifiAccessPointSecurity::Wpa2Personal, 1, &observed,)
            .is_ok()
    );

    let mut open = observed;
    open.wpa2_response_windows = 0;
    assert!(validate_access_point_observation(0, WifiAccessPointSecurity::Open, 1, &open).is_ok());
}

#[test]
fn performance_cycle_omits_unavailable_driver_observation() {
    let report = AccessPointReport {
        schema: ACCESS_POINT_REPORT_SCHEMA,
        fixture_preparation: None,
        boots: vec![BootReport {
            boot: 0,
            error: None,
            cycles: vec![CycleReport {
                cycle: 0,
                traffic: TrafficReport::None,
                secondary_client: None,
                primary_client_link: None,
                secondary_client_link: None,
                independent_air: None,
                access_point: None,
            }],
        }],
    };

    let json = serde_json::to_value(report).unwrap();
    assert_eq!(json["schema"], ACCESS_POINT_REPORT_SCHEMA);
    assert!(json["fixture_preparation"].is_null());
    assert!(json["boots"][0]["cycles"][0]["independent_air"].is_null());
    assert!(
        json["boots"][0]["cycles"][0]
            .get("independent_air_rx")
            .is_none()
    );
    assert!(json["boots"][0]["cycles"][0].get("access_point").is_none());
}

#[test]
fn discovery_ack_misses_cannot_hide_other_or_saturated_tx_failures() {
    assert!(tx_failures_reconciled(0, 0, 0));
    assert!(tx_failures_reconciled(3, 2, 1));
    assert!(!tx_failures_reconciled(4, 2, 1));
    assert!(!tx_failures_reconciled(1, 2, 0));
    assert!(!tx_failures_reconciled(255, 255, 0));
}

#[test]
fn multi_client_fairness_records_the_slowest_flow_and_the_skew_it_checks() {
    let flow = |flow_id: u8, tx_bps| MultiClientFlowReport {
        flow_id,
        peer: Ipv4Endpoint {
            address: [10, 43, 0, flow_id.saturating_add(2)],
            port: 9_002 + u16::from(flow_id),
        },
        rx_bytes: 0,
        tx_bytes: 0,
        rx_units: 0,
        tx_units: 0,
        elapsed_micros: 16_000_000,
        rx_bps: 0,
        tx_bps,
        host_tx_started_at_zero: None,
        host_tx_missing: None,
        host_tx_reordered: None,
        host_tx_duplicates: None,
        host_tx_maximum_interarrival_us: None,
        host_tx_sequence_after_maximum_interarrival: None,
    };
    let criteria = crate::scenario::access_point::MultiClientCriteria {
        exact_delivery: false,
        minimum_rx_bps: None,
        minimum_tx_bps: None,
        minimum_combined_bps: None,
        minimum_bps_per_flow: 48_000_000,
        maximum_flow_skew_percent: Some(15),
        minimum_host_offer_percent: None,
        minimum_secondary_tx_datagrams: None,
        maximum_secondary_tx_interarrival_ms: None,
    };
    let measurements = oer_hil_workload::measurements::Recorder::default();
    // 51 against 60 Mbit/s: exactly 15 % skew passes.
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 60_000_000), flow(1, 51_000_000)],
            Direction::Tx,
            &criteria,
            &measurements,
        )
        .is_ok()
    );
    let recorded = measurements.snapshot();
    let find = |name: &str| recorded.iter().find(|m| m.name == name).unwrap().clone();
    let minimum = find("ap.tx.flow-minimum-rate");
    assert_eq!(minimum.value, 51_000_000);
    assert_eq!(
        minimum.threshold.map(|threshold| threshold.value),
        Some(48_000_000)
    );
    let skew = find("ap.tx.flow-skew");
    assert_eq!(skew.value, 1_500);
    assert_eq!(skew.threshold.map(|threshold| threshold.value), Some(1_500));
    // Just above 15 % fails both the check and its recorded verdict.
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 60_000_000), flow(1, 50_999_999)],
            Direction::Tx,
            &criteria,
            &measurements,
        )
        .is_err()
    );
    assert_eq!(
        measurements
            .snapshot()
            .iter()
            .find(|m| m.name == "ap.tx.flow-skew")
            .unwrap()
            .value,
        1_501
    );
}

#[test]
fn udp_exact_delivery_counts_late_datagrams_and_fails_on_a_loss() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 3 * 1_200,
        datagrams: 3,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let policy = UdpEvidencePolicy {
        exact_delivery: true,
        driver_observation: false,
        rx_delivery: false,
    };
    // Two datagrams in the throughput window and one after it, before the
    // terminal marker: all three delivered.
    let mut late = evidence(2 * 1_200, 0, 2, 0);
    late.transport.rx_late_bytes = 1_200;
    late.transport.rx_late_units = 1;
    late.radio = None;
    assert!(validate_udp(Some(sent), None, late, policy).is_ok());
    // Two in the window and none after it: one lost.
    let mut lost = evidence(2 * 1_200, 0, 2, 0);
    lost.radio = None;
    let error = validate_udp(Some(sent), None, lost, policy).unwrap_err();
    assert!(error.to_string().contains("AP UDP RX mismatch"));
}
