use crate::*;
use oer_hil_protocol::{
    DecodeError, Envelope, FrameDecoder, FrameEncoder, MAX_WIRE_FRAME_BYTES, WireKind,
};
/// Feeds `bytes` and returns the last complete frame as message `M`.
fn receive<M: oer_hil_protocol::Message>(
    decoder: &mut FrameDecoder,
    bytes: &[u8],
) -> Option<Result<Envelope<M>, DecodeError>> {
    let mut last = None;
    decoder.feed(M::WIRE_KIND, bytes, |frame| {
        last = Some(frame.and_then(|frame| {
            frame
                .decode::<M>()
                .expect("the frame carries the expected type")
        }));
    });
    last
}
#[test]
fn asymmetric_bidirectional_session_round_trips() {
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        crate::Configure(SessionConfig {
            network_interface: crate::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [
                Some(crate::SessionFlowConfig {
                    flow_id: 7,
                    peer: Some(Ipv4Endpoint {
                        address: [192, 0, 2, 10],
                        port: 9_002,
                    }),
                    target_rx: Some(FlowConfig {
                        payload_bytes: 1_200,
                        offered_rate_bps: Some(10_000_000),
                        pacing_group_datagrams: None,
                    }),
                    target_tx: Some(FlowConfig {
                        payload_bytes: 1_472,
                        offered_rate_bps: None,
                        pacing_group_datagrams: None,
                    }),
                    payload_identity: None,
                }),
                None,
            ],
            link_requirements: SessionLinkRequirements::tx_block_ack(0),
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn two_peer_udp_session_round_trips_without_erasing_flow_identity() {
    let flow = |flow_id, address| {
        Some(crate::SessionFlowConfig {
            flow_id,
            peer: Some(Ipv4Endpoint {
                address,
                port: 9_002 + u16::from(flow_id),
            }),
            target_rx: Some(FlowConfig {
                payload_bytes: 1_472,
                offered_rate_bps: Some(60_000_000),
                pacing_group_datagrams: None,
            }),
            target_tx: Some(FlowConfig {
                payload_bytes: 1_472,
                offered_rate_bps: Some(60_000_000),
                pacing_group_datagrams: None,
            }),
            payload_identity: None,
        })
    };
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        crate::Configure(SessionConfig {
            network_interface: crate::WifiNetworkInterface::AccessPoint,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [flow(3, [192, 168, 4, 2]), flow(9, [192, 168, 4, 3])],
            link_requirements: SessionLinkRequirements::NONE,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_rx_delivery_evidence_fits_and_round_trips() {
    use crate::{
        EvidenceRecord, RxConsumerLedgerEvidence, RxDeliveryEvidence, RxMacOrderEvidence,
        RxReorderDeliveryEvidence, RxSequenceStageEvidence,
    };

    let stage = RxSequenceStageEvidence {
        data_units: u32::MAX,
        first: Some(u32::MAX),
        highest: Some(u32::MAX),
        gap_events: u32::MAX,
        forward_missing: u32::MAX,
        late_recovered: u32::MAX,
        duplicates: u32::MAX,
        backward_unclassified: u32::MAX,
        first_anomaly: Some(u32::MAX),
        control_markers: u32::MAX,
        data_after_terminal: u32::MAX,
    };
    let delivery = RxDeliveryEvidence {
        post_reorder: stage,
        network_enqueued: stage,
        udp_consumer: stage,
        consumer_ledger: RxConsumerLedgerEvidence {
            matched: u32::MAX,
            enqueued_not_consumed: u32::MAX,
            skipped_before_observed: u32::MAX,
            unexpected_consumer: u32::MAX,
            overflow: u32::MAX,
            first_expected: Some(u32::MAX),
            first_observed: Some(u32::MAX),
        },
        mac_order: RxMacOrderEvidence {
            first_forward_gap: Some(crate::RxForwardGapEvidence {
                previous_udp: u32::MAX,
                current_udp: u32::MAX,
                tid: u8::MAX,
                previous_mac: u16::MAX,
                current_mac: u16::MAX,
            }),
            backward_mac_backward: u32::MAX,
            backward_mac_same: u32::MAX,
            backward_mac_forward: u32::MAX,
            backward_mac_other_tid: u32::MAX,
            backward_mac_unavailable: u32::MAX,
        },
        reorder: RxReorderDeliveryEvidence {
            ingress: u32::MAX,
            ingress_retries: u32::MAX,
            direct: u32::MAX,
            buffered: u32::MAX,
            released: u32::MAX,
            missing: u32::MAX,
            stale: u32::MAX,
            gap_expiries: u32::MAX,
            maximum_occupied: u32::MAX,
            discarded: u32::MAX,
        },
        network_queue_full: u32::MAX,
        network_invalid_length: u32::MAX,
        network_pool_exhausted: u32::MAX,
        network_link_down: u32::MAX,
    };
    let expected = Envelope::new(
        7,
        2,
        9,
        2,
        crate::Evidence(EvidenceRecord::RxDelivery(delivery)),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_network_scheduler_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, NetworkSchedulerEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::Evidence(EvidenceRecord::NetworkScheduler(NetworkSchedulerEvidence {
            polls: u32::MAX,
            ingress_calls: u32::MAX,
            ingress_packets: u32::MAX,
            egress_passes: u32::MAX,
            egress_tx_tokens: u32::MAX,
            egress_blocked: u32::MAX,
            ingress_budget_exhausted: u32::MAX,
            egress_budget_exhausted: u32::MAX,
            started_with_ingress: u32::MAX,
            started_with_egress: u32::MAX,
            exit_drained: u32::MAX,
            exit_work_budget: u32::MAX,
            exit_egress_credit: u32::MAX,
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_zero_copy_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, RxZeroCopyEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::Evidence(EvidenceRecord::RxZeroCopy(RxZeroCopyEvidence {
            cap: u16::MAX,
            adopted: u32::MAX,
            copied_over_cap: u32::MAX,
            copied_unfit: u32::MAX,
            dropped: u32::MAX,
            held_at_end: u16::MAX,
            peak_held: u16::MAX,
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_radio_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, RadioEvidence, RxRadioEvidence, TxRadioEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::Evidence(EvidenceRecord::Radio(RadioEvidence {
            rx: Some(RxRadioEvidence {
                phy_format: u8::MAX,
                ht40_long_gi_frames: u32::MAX,
                ht40_short_gi_frames: u32::MAX,
                ht40_below_mcs7_frames: u32::MAX,
                ht_invalid_frames: u32::MAX,
                dma_buffer_full: u32::MAX,
                dma_fifo_overflow: u32::MAX,
                network_dropped: u32::MAX,
                irq_drain_saturated: u32::MAX,
                unhandled_irq_entries: u32::MAX,
                sequence_first: Some(u32::MAX),
                sequence_highest: Some(u32::MAX),
                sequence_gap_events: u32::MAX,
                sequence_forward_missing: u32::MAX,
                sequence_backward: u32::MAX,
                sequence_duplicates: u32::MAX,
                sequence_unsequenced: u32::MAX,
                s_mpdu_datagrams: u32::MAX,
                not_s_mpdu_datagrams: u32::MAX,
                s_mpdu_unavailable_datagrams: u32::MAX,
                s_mpdu_beacons: u32::MAX,
                not_s_mpdu_beacons: u32::MAX,
                s_mpdu_unavailable_beacons: u32::MAX,
                ampdu_datagrams: u32::MAX,
                not_ampdu_datagrams: u32::MAX,
                hardware_ampdu_datagrams: u32::MAX,
                hardware_not_ampdu_datagrams: u32::MAX,
                protocol_ampdu_datagrams: u32::MAX,
                protocol_not_ampdu_datagrams: u32::MAX,
                ampdu_unavailable_datagrams: u32::MAX,
                reorder_tid: u8::MAX,
                reorder_window: u16::MAX,
                reorder_first_samples: u32::MAX,
                reorder_first_tid: u8::MAX,
                reorder_first_start: u16::MAX,
                reorder_first_sequence: u16::MAX,
                reorder_first_distance: u16::MAX,
                reorder_current_occupied: u32::MAX,
                reorder_maximum_occupied: u32::MAX,
                rx_service_calls: u32::MAX,
                rx_frontier_histogram_samples: u32::MAX,
                mac_irq_entries: u32::MAX,
                mac_irq_classified_entries: u32::MAX,
            }),
            tx: Some(TxRadioEvidence {
                station_terminal: crate::StationTxTerminalEvidence {
                    exchanges: u32::MAX,
                    mpdus: u32::MAX,
                    acknowledged: u32::MAX,
                    unacknowledged: u32::MAX,
                    ordinary_recovered: u32::MAX,
                    ordinary_failed: u32::MAX,
                    invalid_statuses: u32::MAX,
                },
                bandwidth_mhz: u16::MAX,
                aggregate_rate_kbps: u32::MAX,
                aggregates_prepared: u32::MAX,
                publications_pending_start: u32::MAX,
                publications_pending_end: u32::MAX,
                prepared_histogram: [u32::MAX; 8],
                ..TxRadioEvidence::default()
            }),
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_tx_aggregate_timing_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, TxAggregateTimingEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::Evidence(EvidenceRecord::TxAggregateTiming(
            TxAggregateTimingEvidence {
                preparation_micros: u32::MAX,
                preparation_max_micros: u32::MAX,
                publication_micros: u32::MAX,
                publication_max_micros: u32::MAX,
                exchange_micros: u32::MAX,
                exchange_max_micros: u32::MAX,
                first_exchanges: u32::MAX,
                first_exchange_micros: u32::MAX,
                first_exchange_max_micros: u32::MAX,
                retried_exchanges: u32::MAX,
                retry_publications: u32::MAX,
                retry_exchange_micros: u32::MAX,
                retry_exchange_max_micros: u32::MAX,
                tx_irq_epochs: u32::MAX,
                tx_irq_service_samples: u32::MAX,
                tx_irq_clock_skew_samples: u32::MAX,
                tx_irq_service_micros: u32::MAX,
                tx_irq_service_max_micros: u32::MAX,
                tx_publication_to_irq_samples: u32::MAX,
                tx_publication_to_irq_micros: u32::MAX,
                tx_publication_to_irq_max_micros: u32::MAX,
                standby_prepared: u32::MAX,
                standby_published: u32::MAX,
                standby_cancelled: u32::MAX,
                standby_pending_start: u32::MAX,
                standby_pending_end: u32::MAX,
            },
        )),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_flow_transport_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, FlowTransportEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::Evidence(EvidenceRecord::FlowTransport(FlowTransportEvidence {
            rx_maximum_silence_micros: Some(u64::MAX),
            flow_id: u8::MAX,
            rx_bytes: u64::MAX,
            tx_bytes: u64::MAX,
            rx_units: u64::MAX,
            tx_units: u64::MAX,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: u64::MAX,
            transport_errors: u32::MAX,
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn evidence_digest_is_order_and_value_sensitive() {
    use crate::{EvidenceRecord, TransportEvidence};

    let first = EvidenceRecord::Transport(TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 1_200,
        tx_bytes: 0,
        rx_units: 1,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 100,
        transport_errors: 0,
    });
    let second = EvidenceRecord::Transport(TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 2_400,
        ..match first {
            EvidenceRecord::Transport(evidence) => evidence,
            EvidenceRecord::FlowTransport(_)
            | EvidenceRecord::Radio(_)
            | EvidenceRecord::TxAggregateTiming(_)
            | EvidenceRecord::RxDelivery(_)
            | EvidenceRecord::NetworkScheduler(_)
            | EvidenceRecord::Link(_)
            | EvidenceRecord::Stack(_)
            | EvidenceRecord::RxZeroCopy(_) => unreachable!(),
        }
    });

    assert_eq!(evidence_crc32c(&[first]), evidence_crc32c(&[first]));
    assert_ne!(evidence_crc32c(&[first]), evidence_crc32c(&[second]));
    assert_ne!(
        evidence_crc32c(&[first, second]),
        evidence_crc32c(&[second, first])
    );
}
