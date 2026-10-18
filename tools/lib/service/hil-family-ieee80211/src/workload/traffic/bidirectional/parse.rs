use super::*;

pub(super) fn parse_device_report(log: &str) -> DeviceReport {
    let mut report = DeviceReport::default();
    // Every production RX sample is followed by its path, pipeline, IRQ, PHY
    // and task-poll records. Readiness uses the same firmware endpoint and
    // therefore emits short probe samples too; their health counters must not
    // be merged into the sustained interval selected for qualification.
    let mut include_rx_interval_evidence = false;
    for line in log.lines() {
        if line.contains("OAMP aggregates=")
            && let (
                Some(aggregates),
                Some(publications),
                Some(completed),
                Some(subframes),
                Some(acknowledged),
                Some(single),
                Some(individual_retry),
                Some(timeout),
                Some(collision),
                Some(minimum),
                Some(maximum),
                Some(stop_frame),
                Some(stop_capacity),
                Some(stop_empty),
            ) = (
                field(line, "aggregates"),
                field(line, "publications"),
                field(line, "completed"),
                field(line, "subframes"),
                field(line, "acknowledged"),
                field(line, "single"),
                field(line, "individual_retry"),
                field(line, "timeout"),
                field(line, "collision"),
                field(line, "min"),
                field(line, "max"),
                field(line, "stop_frame"),
                field(line, "stop_capacity"),
                field(line, "stop_empty"),
            )
            && let (Ok(minimum), Ok(maximum)) = (u8::try_from(minimum), u8::try_from(maximum))
        {
            report.ampdu.push(AmpduSample {
                aggregates,
                publications,
                completed,
                subframes,
                acknowledged,
                single,
                individual_retry,
                timeout,
                collision,
                minimum,
                maximum,
                stop_frame,
                stop_capacity,
                stop_empty,
            });
        }
        if line.contains("OAMPH one=")
            && let (
                Some(one),
                Some(two_three),
                Some(four_seven),
                Some(eight_fifteen),
                Some(sixteen_twentythree),
                Some(twentyfour_thirty),
                Some(thirtyone),
                Some(full32),
            ) = (
                field(line, "one"),
                field(line, "two_three"),
                field(line, "four_seven"),
                field(line, "eight_fifteen"),
                field(line, "sixteen_twentythree"),
                field(line, "twentyfour_thirty"),
                field(line, "thirtyone"),
                field(line, "full32"),
            )
        {
            report.ampdu_histograms.push(AmpduHistogramSample {
                one,
                two_three,
                four_seven,
                eight_fifteen,
                sixteen_twentythree,
                twentyfour_thirty,
                thirtyone,
                full32,
            });
        }
        if line.contains("OAMPT preparation_us=")
            && let (
                Some(preparation_us),
                Some(preparation_max_us),
                Some(publication_us),
                Some(publication_max_us),
                Some(exchange_us),
                Some(exchange_max_us),
                Some(first_exchanges),
                Some(first_exchange_us),
                Some(first_exchange_max_us),
                Some(retried_exchanges),
                Some(retry_publications),
                Some(retry_exchange_us),
                Some(retry_exchange_max_us),
            ) = (
                field(line, "preparation_us"),
                field(line, "preparation_max_us"),
                field(line, "publication_us"),
                field(line, "publication_max_us"),
                field(line, "exchange_us"),
                field(line, "exchange_max_us"),
                field(line, "first_exchanges"),
                field(line, "first_exchange_us"),
                field(line, "first_exchange_max_us"),
                field(line, "retried_exchanges"),
                field(line, "retry_publications"),
                field(line, "retry_exchange_us"),
                field(line, "retry_exchange_max_us"),
            )
        {
            report.ampdu_timings.push(AmpduTimingSample {
                preparation_us,
                preparation_max_us,
                publication_us,
                publication_max_us,
                exchange_us,
                exchange_max_us,
                first_exchanges,
                first_exchange_us,
                first_exchange_max_us,
                retried_exchanges,
                retry_publications,
                retry_exchange_us,
                retry_exchange_max_us,
            });
        }
        if line.contains("OAMPI tx_irq_epochs=")
            && let (
                Some(tx_irq_epochs),
                Some(tx_irq_samples),
                Some(tx_irq_skew),
                Some(tx_irq_service_us),
                Some(tx_irq_service_max_us),
                Some(tx_flight_samples),
                Some(tx_flight_us),
                Some(tx_flight_max_us),
            ) = (
                field(line, "tx_irq_epochs"),
                field(line, "tx_irq_samples"),
                field(line, "tx_irq_skew"),
                field(line, "tx_irq_service_us"),
                field(line, "tx_irq_service_max_us"),
                field(line, "tx_flight_samples"),
                field(line, "tx_flight_us"),
                field(line, "tx_flight_max_us"),
            )
        {
            report.tx_irq_timings.push(TxIrqTimingSample {
                tx_irq_epochs,
                tx_irq_samples,
                tx_irq_skew,
                tx_irq_service_us,
                tx_irq_service_max_us,
                tx_flight_samples,
                tx_flight_us,
                tx_flight_max_us,
            });
        }
        if (line.starts_with("OAMPB ") || line.contains(" OAMPB "))
            && let (
                Some(samples),
                Some(received),
                Some(success_without),
                Some(nonzero_control),
                Some(start_outside),
                Some(start_lag_max),
                Some(full),
                Some(partial),
                Some(empty),
            ) = (
                field(line, "samples"),
                field(line, "received"),
                field(line, "success_without"),
                field(line, "nonzero_control"),
                field(line, "start_outside"),
                field(line, "start_lag_max"),
                field(line, "full"),
                field(line, "partial"),
                field(line, "empty"),
            )
        {
            report.ampdu_block_acks.push(AmpduBlockAckSample {
                samples,
                received,
                success_without,
                nonzero_control,
                start_outside,
                start_lag_max,
                full,
                partial,
                empty,
            });
        }
        if line.starts_with("ORXQ ") || line.contains(" ORXQ ") {
            let sample = (|| {
                Some(UdpSequenceEvidence {
                    intervals: 1,
                    first: optional_sequence_field(line, "first")?,
                    highest: optional_sequence_field(line, "highest")?,
                    next: optional_sequence_field(line, "next")?,
                    gap_events: field(line, "gap_events")?,
                    forward_missing: field(line, "forward_missing")?,
                    maximum_gap: field(line, "maximum_gap")?,
                    maximum_gap_at: optional_sequence_field(line, "maximum_gap_at")?,
                    first_gap_at: optional_sequence_field(line, "first_gap_at")?,
                    last_gap_at: optional_sequence_field(line, "last_gap_at")?,
                    backward: field(line, "backward")?,
                    adjacent_duplicates: field(line, "adjacent_duplicates")?,
                    unsequenced: field(line, "unsequenced")?,
                    maximum_interarrival_us: field(line, "maximum_interarrival_us")?,
                    maximum_interarrival_at: optional_sequence_field(
                        line,
                        "maximum_interarrival_at",
                    )?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_sequences.push(sample);
            }
        } else if line.starts_with("ORXO ") || line.contains(" ORXO ") {
            let sample = (|| {
                Some(RxOrderEvidence {
                    intervals: 1,
                    gap_events: field(line, "gap_events")?,
                    forward_missing: field(line, "forward_missing")?,
                    backward: field(line, "backward")?,
                    adjacent_duplicates: field(line, "adjacent_duplicates")?,
                    backward_mac_backward: field(line, "backward_mac_backward")?,
                    backward_mac_same: field(line, "backward_mac_same")?,
                    backward_mac_forward: field(line, "backward_mac_forward")?,
                    backward_mac_other_tid: field(line, "backward_mac_other_tid")?,
                    backward_mac_unavailable: field(line, "backward_mac_unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_order.push(sample);
            }
        } else if line.starts_with("ORTP ") || line.contains(" ORTP ") {
            if include_rx_interval_evidence {
                report.task_polls.merge_log_line(line);
            }
        } else if line.starts_with("ORXS ") || line.contains(" ORXS ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    service_calls: field(line, "calls")?,
                    frontier_frames: field(line, "frontier")?,
                    admitted_frames: field(line, "admitted")?,
                    staged_bytes: field(line, "bytes")?,
                    stage_empty_discards: field(line, "discard_empty").unwrap_or(0),
                    stage_too_long_discards: field(line, "discard_long").unwrap_or(0),
                    maximum_frontier: field(line, "fmax")?,
                    maximum_admitted: field(line, "amax")?,
                    service_us: field(line, "service_us")?,
                    service_max_us: field(line, "service_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXSC ") || line.contains(" ORXSC ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    service_credit_samples: field(line, "samples")?,
                    backpressured_services: field(line, "back")?,
                    bulk_capacity_blocked_services: field(line, "bulk_blocked")?,
                    pool_credit_limited_services: field(line, "pool")?,
                    queue_credit_limited_services: field(line, "queue")?,
                    maximum_deferred_frames: field(line, "deferred_max")?,
                    minimum_backpressured_pool_credits: field(line, "pool_min")?,
                    minimum_backpressured_queue_credits: field(line, "queue_min")?,
                    minimum_pool_credits: field(line, "pool_floor")?,
                    minimum_queue_credits: field(line, "queue_floor")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXD ") || line.contains(" ORXD ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    protocol_frames: field(line, "frames")?,
                    protocol_data_frames: field(line, "data")?,
                    protocol_amsdu_mpdus: field(line, "amsdu").unwrap_or(0),
                    protocol_amsdu_subframes: field(line, "amsdu_subframes").unwrap_or(0),
                    protocol_units_le_1700: field(line, "unit_le1700").unwrap_or(0),
                    protocol_units_1701_3400: field(line, "unit_1701_3400").unwrap_or(0),
                    protocol_units_over_3400: field(line, "unit_over3400").unwrap_or(0),
                    protocol_unit_max_bytes: field(line, "unit_boot_max_bytes").unwrap_or(0),
                    network_ready_waits: field(line, "waits")?,
                    network_ready_wait_us: field(line, "wait_us")?,
                    network_ready_wait_max_us: field(line, "wait_boot_max_us")?,
                    dispatch_us: field(line, "dispatch_us")?,
                    dispatch_max_us: field(line, "dispatch_boot_max_us")?,
                    network_publications: field(line, "publications")?,
                    network_published_bytes: field(line, "bytes")?,
                    network_publish_us: field(line, "publish_us")?,
                    network_publish_max_us: field(line, "publish_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_dispatch.push(sample);
            }
            if include_rx_interval_evidence
                && report.software_health.len() < report.rx.len()
                && let (Some(enqueued), Some(dropped)) =
                    (field(line, "enqueued"), field(line, "dropped"))
            {
                report.software_health.push((enqueued, dropped));
            }
        } else if line.starts_with("ORXB ") || line.contains(" ORXB ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    dma_buffer_full_increments: field(line, "increments")?,
                    dma_buffer_full_service_samples: field(line, "samples")?,
                    dma_buffer_full_between_services: field(line, "between").unwrap_or(0),
                    dma_buffer_full_during_services: field(line, "during").unwrap_or(0),
                    dma_buffer_full_between_service_samples: field(line, "between_samples")
                        .unwrap_or(0),
                    dma_buffer_full_during_service_samples: field(line, "during_samples")
                        .unwrap_or(0),
                    dma_buffer_full_last_service: field(line, "last_service")?,
                    dma_buffer_full_last_phase: field(line, "last_phase").unwrap_or(0),
                    dma_buffer_full_last_counter: field(line, "last_counter")?,
                    dma_buffer_full_last_frontier: field(line, "last_frontier")?,
                    dma_buffer_full_last_admitted: field(line, "last_admitted")?,
                    dma_buffer_full_last_pool_credits: field(line, "last_pool")?,
                    dma_buffer_full_last_queue_credits: field(line, "last_queue")?,
                    dma_buffer_full_last_service_us: field(line, "last_service_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXL ") || line.contains(" ORXL ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    reload_transactions: field(line, "transactions")?,
                    reload_us: field(line, "reload_us")?,
                    reload_max_us: field(line, "reload_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_service.push(sample);
            }
        } else if line.starts_with("ORXR ") || line.contains(" ORXR ") {
            let sample = (|| {
                Some(RxReorderEvidence {
                    intervals: 1,
                    starts: field(line, "starts")?,
                    stops: field(line, "stops")?,
                    last_start_tid: field(line, "start_tid")?,
                    last_start_sequence: field(line, "start_seq")?,
                    window: field(line, "window")?,
                    first_samples: field(line, "first_samples")?,
                    first_tid: field(line, "first_tid")?,
                    first_start_sequence: field(line, "first_start")?,
                    first_frame_sequence: field(line, "first_seq")?,
                    first_distance: field(line, "first_distance")?,
                    buffered: field(line, "buffered")?,
                    released: field(line, "released")?,
                    missing: field(line, "missing")?,
                    stale: field(line, "stale")?,
                    gap_expiries: field(line, "expiries")?,
                    occupied: field(line, "occupied")?,
                    maximum_occupied: field(line, "occupied_max")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_reorder.push(sample);
            }
        } else if line.starts_with("ORXF ") || line.contains(" ORXF ") {
            let sample = (|| {
                Some(RxPipelineEvidence {
                    frontier_zero_services: field(line, "zero")?,
                    frontier_one_services: field(line, "one")?,
                    frontier_two_three_services: field(line, "two_three")?,
                    frontier_four_seven_services: field(line, "four_seven")?,
                    frontier_eight_fifteen_services: field(line, "eight_fifteen")?,
                    frontier_sixteen_thirty_one_services: field(line, "sixteen_thirty_one")?,
                    frontier_thirty_two_plus_services: field(line, "thirty_two_plus")?,
                    rx_irq_posts: field(line, "irq_posts")?,
                    rx_irq_epochs: field(line, "irq_epochs")?,
                    mac_irq_entries: field(line, "irq_entries").unwrap_or(0),
                    rx_irq_coalesced_posts: field(line, "irq_coalesced")?,
                    rx_irq_service_samples: field(line, "irq_samples")?,
                    rx_irq_clock_skew_samples: field(line, "irq_skew")?,
                    rx_irq_to_service_us: field(line, "irq_service_us")?,
                    rx_irq_to_service_max_us: field(line, "irq_service_boot_max_us")?,
                    ..RxPipelineEvidence::default()
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_frontier.push(sample);
            }
        } else if line.starts_with("ORXI ") || line.contains(" ORXI ") {
            let sample = (|| {
                Some(MacIrqEvidence {
                    spurious_entries: field(line, "spurious")?,
                    rx_only_entries: field(line, "rx_only")?,
                    rx_mixed_entries: field(line, "rx_mixed")?,
                    tx_only_entries: field(line, "tx_only")?,
                    tx_mixed_entries: field(line, "tx_mixed")?,
                    other_only_entries: field(line, "other_only")?,
                    extra_nonzero_snapshots: field(line, "extra")?,
                    saturated_entries: field(line, "saturated")?,
                    auxiliary_entries: field(line, "aux_entries")?,
                    unhandled_entries: field(line, "unhandled_entries")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.mac_irq.push(sample);
            }
        } else if line.starts_with("ORXSM ") || line.contains(" ORXSM ") {
            let sample = (|| {
                Some(RxSmpduEvidence {
                    s_mpdu_datagrams: field(line, "s_mpdu")?,
                    not_s_mpdu_datagrams: field(line, "not_s_mpdu")?,
                    unavailable_datagrams: field(line, "unavailable")?,
                    s_mpdu_beacons: field(line, "beacon_s_mpdu")?,
                    not_s_mpdu_beacons: field(line, "beacon_not_s_mpdu")?,
                    unavailable_beacons: field(line, "beacon_unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_s_mpdu.push(sample);
            }
        } else if line.starts_with("ORXAG ") || line.contains(" ORXAG ") {
            let sample = (|| {
                Some(RxAmpduEvidence {
                    ampdu_datagrams: field(line, "ampdu")?,
                    not_ampdu_datagrams: field(line, "not_ampdu")?,
                    hardware_ampdu_datagrams: field(line, "hardware_ampdu")?,
                    hardware_not_ampdu_datagrams: field(line, "hardware_not_ampdu")?,
                    protocol_ampdu_datagrams: field(line, "protocol_ampdu")?,
                    protocol_not_ampdu_datagrams: field(line, "protocol_not_ampdu")?,
                    unavailable_datagrams: field(line, "unavailable")?,
                })
            })();
            if include_rx_interval_evidence && let Some(sample) = sample {
                report.rx_ampdu.push(sample);
            }
        } else if line.starts_with("ORXM ") || line.contains(" ORXM ") {
            let histogram = core::array::from_fn(|mcs| field(line, &format!("m{mcs}")));
            if include_rx_interval_evidence
                && histogram.iter().all(Option::is_some)
                && let Some(other) = field(line, "other")
            {
                report
                    .rx_mcs_histograms
                    .push((histogram.map(Option::unwrap), other));
            }
        } else if line.starts_with("ORXHTL ") || line.contains(" ORXHTL ") {
            let histogram = core::array::from_fn(|mcs| field(line, &format!("m{mcs}")));
            if include_rx_interval_evidence && histogram.iter().all(Option::is_some) {
                report
                    .rx_ht40_long_gi_histograms
                    .push(histogram.map(Option::unwrap));
            }
        } else if line.starts_with("ORXHTS ") || line.contains(" ORXHTS ") {
            let histogram = core::array::from_fn(|mcs| field(line, &format!("m{mcs}")));
            if include_rx_interval_evidence && histogram.iter().all(Option::is_some) {
                report
                    .rx_ht40_short_gi_histograms
                    .push(histogram.map(Option::unwrap));
            }
        } else if line.starts_with("ORXHTW ") || line.contains(" ORXHTW ") {
            let histogram = core::array::from_fn(|mcs| field(line, &format!("m{mcs}")));
            if include_rx_interval_evidence
                && histogram.iter().all(Option::is_some)
                && let Some(other) = field(line, "other")
            {
                report
                    .rx_ht20_histograms
                    .push((histogram.map(Option::unwrap), other));
            }
        } else if line.starts_with("ORX ") || line.contains(" ORX ") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) =
                (field(line, "d"), field(line, "u"), field(line, "k"))
            {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
                if let Some(address) = field(line, "code") {
                    report.code_addresses.push(address);
                }
                if report.dma_health.len() < report.rx.len()
                    && let (Some(buffer_full), Some(fifo_overflow)) =
                        (field(line, "full"), field(line, "overflow"))
                {
                    report.dma_health.push((buffer_full, fifo_overflow));
                }
            }
        } else if line.contains("OTX ") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) =
                (field(line, "k"), field(line, "w"), field(line, "r"))
            {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
                if let Some(address) = field(line, "a").or_else(|| field(line, "code")) {
                    report.code_addresses.push(address);
                }
            }
        } else if line.starts_with("ORXP ") || line.contains(" ORXP ") {
            if include_rx_interval_evidence && let Some(format) = field(line, "f") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("result=BENCH") && line.contains("stage=udp-rx-direct") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) = (
                field(line, "datagrams"),
                field(line, "elapsed_us"),
                field(line, "throughput_kbps"),
            ) {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
            }
        } else if line.contains("result=BENCH") && has_token(line, "stage=udp-rx") {
            include_rx_interval_evidence = false;
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) = (
                field(line, "datagrams"),
                field(line, "elapsed_us"),
                field(line, "throughput_kbps"),
            ) {
                let sample = ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                };
                include_rx_interval_evidence = is_qualified_rx_sample(sample);
                report.rx.push(sample);
            }
            if let Some(address) = field(line, "code_address") {
                report.code_addresses.push(address);
            }
        } else if line.contains("result=BENCH") && has_token(line, "stage=udp-rx-path") {
            if include_rx_interval_evidence
                && let (Some(buffer_full), Some(fifo_overflow)) =
                    (field(line, "buffer_full"), field(line, "fifo_overflow"))
            {
                report.dma_health.push((buffer_full, fifo_overflow));
            }
            if include_rx_interval_evidence
                && let (Some(enqueued), Some(dropped)) =
                    (field(line, "enqueued"), field(line, "queue_dropped"))
            {
                report.software_health.push((enqueued, dropped));
            }
            if include_rx_interval_evidence
                && let Some(format) =
                    field(line, "rx_format").filter(|format| *format <= u8::MAX.into())
            {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("result=BENCH") && line.contains("stage=raw-mac-tx") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) = (
                field(line, "throughput_kbps"),
                field(line, "bandwidth_mhz"),
                field(line, "rate_kbps"),
            ) {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
            }
        } else if line.contains("stage=udp-rx-phy") {
            if include_rx_interval_evidence && let Some(format) = field(line, "rx_format") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("stage=rx-runtime-delta") {
            if let (Some(buffer_full), Some(fifo_overflow)) =
                (field(line, "buffer_full"), field(line, "fifo_overflow"))
            {
                report.dma_health.push((buffer_full, fifo_overflow));
            }
        } else if line.contains("stage=tx-runtime")
            && let Some(address) = field(line, "code_address")
        {
            report.code_addresses.push(address);
        }
        if line.contains("result=FAIL")
            && (line.contains("raw-mac")
                || line.contains("embassy-net-radio")
                || line.contains("rx-output-reservation")
                || line.contains("mic-failure"))
        {
            report.failures.push(line.to_owned());
        }
    }
    report
}

pub(super) fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.trim_end_matches(',').parse::<u64>().ok())?
    })
}

fn optional_sequence_field(line: &str, key: &str) -> Option<Option<u64>> {
    let value = field(line, key)?;
    Some((value != u64::from(u32::MAX)).then_some(value))
}

pub(super) fn text_field<'line>(line: &'line str, key: &str) -> Option<&'line str> {
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.trim_end_matches(','))
    })
}

fn has_token(line: &str, expected: &str) -> bool {
    line.split_whitespace().any(|token| token == expected)
}
