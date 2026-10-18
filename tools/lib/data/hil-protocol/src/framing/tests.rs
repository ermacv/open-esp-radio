use super::*;
use crate::network::evidence_crc32c;

/// Feeds `bytes` and returns the last complete frame as message `M`.
fn receive<M: crate::Message>(
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
use crate::system::InjectPanic;
use crate::{
    Envelope, WireKind, network::Completion, network::Direction, network::FlowConfig,
    network::Ipv4Endpoint, network::SessionConfig, network::SessionLinkRequirements,
    network::Transport,
};

fn command(sequence: u32) -> Envelope<InjectPanic> {
    Envelope::new(0x1234_5678_9abc_def0, sequence, 42, sequence, InjectPanic)
}

#[test]
fn round_trips_one_byte_at_a_time() {
    let expected = command(7);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    for byte in frame {
        if let Some(message) = receive(&mut decoder, core::slice::from_ref(byte)) {
            observed = Some(message.unwrap());
        }
    }
    assert_eq!(observed, Some(expected));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn wire_header_is_fixed_and_precedes_the_postcard_body() {
    let expected = command(7);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut raw = [0_u8; MAX_RAW_FRAME_BYTES];
    let decoded = cobs::decode(&frame[2..frame.len() - 1], &mut raw).unwrap();

    assert_eq!(&raw[..4], &WIRE_MAGIC);
    assert_eq!(raw[4], FRAMING_VERSION);
    assert_eq!(raw[5], WireKind::Command as u8);
    assert_eq!(
        u64::from_le_bytes(raw[6..14].try_into().unwrap()),
        expected.boot_id
    );
    assert_eq!(
        u32::from_le_bytes(raw[14..18].try_into().unwrap()),
        expected.message_sequence
    );
    assert_eq!(raw[KEY_RANGE], <InjectPanic as crate::Message>::KEY.0);
    let payload_length = usize::from(u16::from_le_bytes(raw[LENGTH_RANGE].try_into().unwrap()));
    assert_eq!(decoded, WIRE_HEADER_BYTES + payload_length + CHECKSUM_BYTES);
    assert_eq!(
        postcard::from_bytes::<InjectPanic>(
            &raw[WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + payload_length]
        )
        .unwrap(),
        expected.body
    );
}

#[test]
fn a_frame_of_another_type_is_not_decoded_as_this_one() {
    let mut encoder = FrameEncoder::new();
    let frame = encoder
        .encode(&Envelope::new(7, 1, 0, 1, crate::base::GetBootStatus))
        .unwrap();
    let mut decoder = FrameDecoder::new();
    let mut seen = None;
    decoder.feed(WireKind::Command, frame, |result| {
        let frame = result.unwrap();
        assert_eq!(
            frame.key,
            <crate::base::GetBootStatus as crate::Message>::KEY
        );
        seen = Some(frame.decode::<InjectPanic>().is_none());
    });
    assert_eq!(seen, Some(true));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn rejects_an_event_on_the_command_endpoint() {
    let expected = Envelope::new(7, 1, 0, 1, crate::base::Accepted);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive::<InjectPanic>(&mut decoder, frame);
    assert_eq!(observed, Some(Err(DecodeError::MessageKind)));
    assert_eq!(decoder.counters().message_kind_errors, 1);
    assert_eq!(decoder.counters().deserialize_errors, 0);
}

#[test]
fn leading_delimiter_recovers_from_text_output() {
    let expected = command(9);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    const NOISE: &[u8] = b"rom boot text\n";
    let mut input = [0_u8; MAX_WIRE_FRAME_BYTES + NOISE.len()];
    input[..NOISE.len()].copy_from_slice(NOISE);
    input[NOISE.len()..NOISE.len() + frame.len()].copy_from_slice(frame);

    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(
        WireKind::Command,
        &input[..NOISE.len() + frame.len()],
        |result| {
            if let Ok(frame) = result {
                observed = frame.decode::<InjectPanic>().map(Result::unwrap);
            }
        },
    );
    assert_eq!(observed, Some(expected));
}

#[test]
fn rejects_checksum_corruption_and_recovers_for_next_frame() {
    let first = command(1);
    let second = command(2);
    let mut encoder = FrameEncoder::new();
    let mut damaged = [0_u8; MAX_WIRE_FRAME_BYTES];
    let first_frame = encoder.encode(&first).unwrap();
    damaged[..first_frame.len()].copy_from_slice(first_frame);
    let damaged_length = first_frame.len();
    damaged[damaged_length - 3] ^= 0x40;
    let second_frame = encoder.encode(&second).unwrap();

    let mut decoder = FrameDecoder::new();
    let mut errors = 0;
    decoder.feed(WireKind::Command, &damaged[..damaged_length], |result| {
        errors += usize::from(result.is_err());
    });
    let observed = receive(&mut decoder, second_frame).map(Result::unwrap);
    assert_eq!(errors, 1);
    assert_eq!(observed, Some(second));
}

#[test]
fn discards_overfull_noise_until_a_delimiter() {
    let expected = command(3);
    let mut decoder = FrameDecoder::new();
    decoder.feed(WireKind::Command, &[0], |_| {});
    decoder.feed(WireKind::Command, &[0x55; MAX_COBS_FRAME_BYTES + 4], |_| {});
    decoder.feed(WireKind::Command, &[0], |_| {});

    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
    assert_eq!(decoder.counters().overflows, 1);
}

#[test]
fn asymmetric_bidirectional_session_round_trips() {
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        crate::network::Configure(SessionConfig {
            network_interface: crate::network::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [
                Some(crate::network::SessionFlowConfig {
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
        Some(crate::network::SessionFlowConfig {
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
        crate::network::Configure(SessionConfig {
            network_interface: crate::network::WifiNetworkInterface::AccessPoint,
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
        network::EvidenceRecord, network::RxConsumerLedgerEvidence, network::RxDeliveryEvidence,
        network::RxMacOrderEvidence, network::RxReorderDeliveryEvidence,
        network::RxSequenceStageEvidence,
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
            first_forward_gap: Some(crate::network::RxForwardGapEvidence {
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
        crate::network::Evidence(EvidenceRecord::RxDelivery(delivery)),
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
    use crate::{network::EvidenceRecord, network::NetworkSchedulerEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::NetworkScheduler(NetworkSchedulerEvidence {
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
    use crate::{network::EvidenceRecord, network::RxZeroCopyEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::RxZeroCopy(RxZeroCopyEvidence {
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
    use crate::{
        network::EvidenceRecord, network::RadioEvidence, network::RxRadioEvidence,
        network::TxRadioEvidence,
    };

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::Radio(RadioEvidence {
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
                station_terminal: crate::network::StationTxTerminalEvidence {
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
    use crate::{network::EvidenceRecord, network::TxAggregateTimingEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::TxAggregateTiming(
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
    use crate::{network::EvidenceRecord, network::FlowTransportEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::FlowTransport(FlowTransportEvidence {
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
    use crate::{network::EvidenceRecord, network::TransportEvidence};

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

#[test]
fn an_intact_frame_whose_payload_is_not_its_type_is_a_payload_error() {
    // Equal keys mean equal schemas, so only a defective sender produces
    // this: an intact frame whose body is no value of the type it names.
    let expected = Envelope::new(
        0x1234_5678_9abc_def0,
        9,
        42,
        9,
        crate::system::WatchdogTest(crate::system::WatchdogTestMode::Complete),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let encoded = &frame[2..frame.len() - 1];
    let mut raw = [0u8; 64];
    raw[..encoded.len()].copy_from_slice(encoded);
    let length = cobs::decode_in_place(&mut raw[..encoded.len()]).unwrap();
    let protected = length - CHECKSUM_BYTES;
    assert_eq!(
        protected,
        WIRE_HEADER_BYTES + 1,
        "a watchdog test mode is one variant byte"
    );
    raw[WIRE_HEADER_BYTES] = 0x7f;
    let checksum = CRC32C.checksum(&raw[..protected]).to_le_bytes();
    raw[protected..length].copy_from_slice(&checksum);
    let mut reencoded = [0u8; 80];
    reencoded[1] = 0;
    let written = cobs::encode(&raw[..length], &mut reencoded[2..]);
    let frame = &reencoded[..written + 3];

    let mut decoder = FrameDecoder::new();
    let observed = receive::<crate::system::WatchdogTest>(&mut decoder, frame);
    assert_eq!(observed, Some(Err(DecodeError::Payload)));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn a_message_serialized_by_its_producer_frames_like_an_encoded_one() {
    let expected = command(11);
    let mut encoder = FrameEncoder::new();
    let direct = encoder.encode(&expected).unwrap().to_vec();
    let outbound = Outbound::new(
        expected.message_sequence,
        expected.session_id,
        expected.request_id,
        &expected.body,
    )
    .unwrap();
    assert!(outbound.is::<InjectPanic>());
    let queued = encoder
        .encode_outbound(expected.boot_id, &outbound)
        .unwrap()
        .to_vec();
    assert_eq!(queued, direct);
}
