use oer_hil_protocol::{
    network::Direction, network::Finished, network::FlowTransportEvidence, network::RadioEvidence,
    network::ResultSummary, network::RxRadioEvidence, network::RxZeroCopyEvidence,
    network::SessionLinkRequirements, network::SessionReady, network::TransportEvidence,
    system::StackUsage, system::StackWatermark,
};

use crate::{
    SessionEvidence, session::session_ready_covers, validate_stack_usage,
    validation::validate_rx_zero_copy,
};

mod capture;

fn session_with_rx(rx: RxRadioEvidence) -> SessionEvidence {
    let transport = TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 0,
        tx_bytes: 0,
        rx_units: 0,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 1,
        transport_errors: 0,
    };
    SessionEvidence {
        transport,
        flow_transport: [
            Some(FlowTransportEvidence::from_session_total(0, transport)),
            None,
        ],
        radio: Some(RadioEvidence {
            rx: Some(rx),
            tx: None,
        }),
        tx_timing: None,
        rx_delivery: None,
        network_scheduler: None,
        rx_zero_copy: None,
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
                evidence_records: 4,
            },
            evidence_crc32c: 0,
        },
    }
}

fn healthy_he_rx() -> RxRadioEvidence {
    RxRadioEvidence {
        phy_format: 4,
        sequence_first: Some(0),
        sequence_highest: Some(99),
        not_s_mpdu_datagrams: 100,
        not_s_mpdu_beacons: 1,
        ampdu_datagrams: 100,
        protocol_ampdu_datagrams: 100,
        reorder_tid: 0,
        reorder_window: 64,
        reorder_first_samples: 1,
        reorder_first_tid: 0,
        reorder_first_start: 7,
        reorder_first_sequence: 9,
        reorder_first_distance: 2,
        reorder_maximum_occupied: 8,
        rx_service_calls: 10,
        rx_frontier_histogram_samples: 10,
        mac_irq_entries: 10,
        mac_irq_classified_entries: 10,
        ..RxRadioEvidence::default()
    }
}

fn healthy_ht_rx() -> RxRadioEvidence {
    RxRadioEvidence {
        phy_format: 2,
        sequence_first: Some(0),
        sequence_highest: Some(99),
        not_s_mpdu_datagrams: 100,
        not_s_mpdu_beacons: 1,
        ampdu_datagrams: 100,
        hardware_ampdu_datagrams: 100,
        reorder_tid: 0,
        reorder_window: 64,
        reorder_first_samples: 1,
        reorder_first_tid: 0,
        reorder_first_start: 7,
        reorder_first_sequence: 9,
        reorder_first_distance: 2,
        reorder_maximum_occupied: 8,
        rx_service_calls: 10,
        rx_frontier_histogram_samples: 10,
        mac_irq_entries: 10,
        mac_irq_classified_entries: 10,
        ..RxRadioEvidence::default()
    }
}

#[test]
fn typed_rx_radio_enforces_order_and_provenance_without_text() {
    assert!(
        session_with_rx(healthy_he_rx())
            .require_rx_radio(4, 100)
            .is_ok()
    );

    let mut reordered = healthy_he_rx();
    reordered.sequence_backward = 1;
    assert!(
        session_with_rx(reordered)
            .require_rx_radio_health(4)
            .is_ok(),
        "performance evidence keeps radio-health guarantees without claiming exact delivery",
    );
    assert!(session_with_rx(reordered).require_rx_radio(4, 100).is_err());

    let mut wrong_provenance = healthy_he_rx();
    wrong_provenance.protocol_ampdu_datagrams = 0;
    wrong_provenance.hardware_ampdu_datagrams = 100;
    assert!(
        session_with_rx(wrong_provenance)
            .require_rx_radio_health(4)
            .is_err()
    );
}

#[test]
fn zero_copy_evidence_rejects_holdings_above_the_cap() {
    let within = RxZeroCopyEvidence {
        cap: 16,
        adopted: 90,
        copied_over_cap: 10,
        held_at_end: 4,
        peak_held: 16,
        ..RxZeroCopyEvidence::default()
    };
    assert!(validate_rx_zero_copy(within).is_ok());
    for invalid in [
        RxZeroCopyEvidence {
            peak_held: 17,
            ..within
        },
        RxZeroCopyEvidence {
            held_at_end: 5,
            peak_held: 4,
            ..within
        },
        RxZeroCopyEvidence { cap: 0, ..within },
    ] {
        assert!(validate_rx_zero_copy(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn typed_ht_rx_allows_protocol_proven_non_aggregated_fallback() {
    let mut mixed = healthy_ht_rx();
    mixed.not_ampdu_datagrams = 2;
    mixed.protocol_not_ampdu_datagrams = 2;
    assert!(session_with_rx(mixed).require_rx_radio_health(2).is_ok());

    let mut protocol_positive = healthy_ht_rx();
    protocol_positive.hardware_ampdu_datagrams = 99;
    protocol_positive.protocol_ampdu_datagrams = 1;
    assert!(
        session_with_rx(protocol_positive)
            .require_rx_radio_health(2)
            .is_err()
    );

    let mut unavailable = healthy_ht_rx();
    unavailable.ampdu_unavailable_datagrams = 1;
    assert!(
        session_with_rx(unavailable)
            .require_rx_radio_health(2)
            .is_err()
    );

    let mut no_aggregate = healthy_ht_rx();
    no_aggregate.ampdu_datagrams = 0;
    no_aggregate.hardware_ampdu_datagrams = 0;
    no_aggregate.not_ampdu_datagrams = 100;
    no_aggregate.hardware_not_ampdu_datagrams = 100;
    assert!(
        session_with_rx(no_aggregate)
            .require_rx_radio_health(2)
            .is_err()
    );
}

#[test]
fn runtime_stack_policy_rejects_low_headroom_on_either_core() {
    let watermark = StackWatermark {
        capacity_bytes: 16_000,
        free_bytes: 4_000,
        used_bytes: 12_000,
        minimum_free_bytes: 4_000,
    };
    assert!(
        validate_stack_usage(StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
            cpu0: watermark,
            cpu1: watermark,
        })
        .is_ok()
    );
    let insufficient = StackWatermark {
        minimum_free_bytes: 4_001,
        ..watermark
    };
    assert!(
        validate_stack_usage(StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
            cpu0: watermark,
            cpu1: insufficient,
        })
        .is_err()
    );
}

#[test]
fn bidirectional_readiness_covers_both_owned_data_planes() {
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: Some(0),
        },
        Direction::Rx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: Some(0),
        },
        Direction::Tx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Rx,
            tx_block_ack_tid: None,
        },
        Direction::Rx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(!session_ready_covers(
        Direction::Rx,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: None,
        },
        Direction::Rx,
        SessionLinkRequirements::NONE,
    ));
    assert!(!session_ready_covers(
        Direction::Tx,
        SessionReady {
            direction: Direction::Tx,
            tx_block_ack_tid: None,
        },
        Direction::Tx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
}

#[test]
fn tx_radio_requires_exact_balance_including_interval_boundary_owners() {
    use oer_hil_protocol::{network::TxAggregateTimingEvidence, network::TxRadioEvidence};
    let check = |publications, pending_start, pending_end| {
        let mut session = session_with_rx(RxRadioEvidence::default());
        session.radio.as_mut().unwrap().tx = Some(TxRadioEvidence {
            bandwidth_mhz: 20,
            aggregate_rate_kbps: 114700,
            aggregates_prepared: 1,
            aggregate_publications: publications,
            publications_pending_start: pending_start,
            publications_pending_end: pending_end,
            aggregates_completed: 1,
            subframes_prepared: 2,
            subframes_acknowledged: 2,
            minimum_subframes: 2,
            maximum_subframes: 2,
            prepared_histogram: [0, 1, 0, 0, 0, 0, 0, 0],
            stopped_on_empty_queue: 1,
            block_ack_samples: 1,
            block_ack_received: 1,
            full_block_ack: 1,
            ..Default::default()
        });
        session.tx_timing = Some(TxAggregateTimingEvidence {
            preparation_micros: 1,
            preparation_max_micros: 1,
            publication_micros: publications,
            publication_max_micros: u32::from(publications != 0),
            exchange_micros: 1,
            exchange_max_micros: 1,
            first_exchanges: 1,
            first_exchange_micros: 1,
            first_exchange_max_micros: 1,
            ..Default::default()
        });
        session.require_tx_radio(20, 100000, 1)
    };
    assert!(check(1, 0, 0).is_ok());
    assert!(check(2, 0, 1).is_ok()); // Last publication is still outstanding.
    assert!(check(0, 1, 0).is_ok()); // Completion belongs to preceding interval.
    assert!(check(2, 0, 0).is_err()); // Lost completion remains an error.
    assert!(check(1, 0, 1).is_err()); // Inventing an owner is also an error.
}

#[test]
fn irq_stack_reserve_is_checked_independently_on_each_hart() {
    let healthy = StackWatermark {
        capacity_bytes: 32768,
        free_bytes: 4096,
        used_bytes: 28672,
        minimum_free_bytes: 4096,
    };
    let mut usage = StackUsage {
        cpu0: healthy,
        cpu1: healthy,
        cpu0_irq: Some(healthy),
        cpu1_irq: Some(healthy),
    };
    assert!(validate_stack_usage(usage).is_ok());
    for cpu in [0, 1] {
        let bad = StackWatermark {
            free_bytes: 4095,
            used_bytes: 28673,
            ..healthy
        };
        if cpu == 0 {
            usage.cpu0_irq = Some(bad);
        } else {
            usage.cpu1_irq = Some(bad);
        }
        let error = validate_stack_usage(usage).unwrap_err().to_string();
        assert!(error.contains(&format!("cpu{cpu}-irq")));
        usage.cpu0_irq = Some(healthy);
        usage.cpu1_irq = Some(healthy);
    }
    usage.cpu1_irq.as_mut().unwrap().used_bytes = 0;
    assert!(
        validate_stack_usage(usage)
            .unwrap_err()
            .to_string()
            .contains("inconsistent cpu1-irq")
    );
    usage.cpu1_irq = None;
    assert!(validate_stack_usage(usage).is_err());
}
