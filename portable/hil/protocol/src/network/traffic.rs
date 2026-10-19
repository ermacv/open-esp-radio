//! A traffic session's network, transport, radio and delivery evidence.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use super::WifiNetworkInterface;
use super::{Direction, Transport};

/// Maximum number of independently accounted transport flows in one network
/// interface session.
///
/// Two flows are sufficient for the first physical multi-client AP cell. The
/// fixed bound keeps the no-alloc wire contract explicit and does not change
/// the target rule that one session owns one network interface.
pub const SESSION_FLOW_CAPACITY: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct NetworkInfo {
    pub network_interface: WifiNetworkInterface,
    pub address: [u8; 4],
    pub prefix_length: u8,
    pub gateway: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ServiceInfo {
    pub network_interface: WifiNetworkInterface,
    pub transport: Transport,
    pub direction: Direction,
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TransportEvidence {
    /// Complete observation-window silence, including its trailing interval.
    /// Present only for a single measured UDP RX flow; never inferred from throughput.
    pub rx_maximum_silence_micros: Option<u64>,
    /// Received payload inside the measurement window: the throughput's.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    /// Received payload after the measurement window, before the peer's
    /// terminal marker ended the session: delivered, outside the
    /// throughput.
    pub rx_late_bytes: u64,
    pub rx_late_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

impl TransportEvidence {
    /// Every datagram delivered before the peer's terminal marker, in the
    /// measurement window or after it: what an exact delivery compares with
    /// the peer's count.
    pub const fn rx_delivered_units(&self) -> u64 {
        self.rx_units.saturating_add(self.rx_late_units)
    }

    pub const fn rx_delivered_bytes(&self) -> u64 {
        self.rx_bytes.saturating_add(self.rx_late_bytes)
    }
}

/// Transport accounting for one configured [`SessionFlowConfig`](super::SessionFlowConfig).
///
/// The session-wide [`TransportEvidence`] remains the independently checked
/// sum used by existing ceiling reports. Fairness verdicts consume these
/// records and must never infer a peer split from the aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FlowTransportEvidence {
    pub rx_maximum_silence_micros: Option<u64>,
    pub flow_id: u8,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    /// As [`TransportEvidence::rx_late_bytes`], for this flow.
    pub rx_late_bytes: u64,
    pub rx_late_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

impl FlowTransportEvidence {
    /// As [`TransportEvidence::rx_delivered_units`], for this flow.
    pub const fn rx_delivered_units(&self) -> u64 {
        self.rx_units.saturating_add(self.rx_late_units)
    }

    pub const fn rx_delivered_bytes(&self) -> u64 {
        self.rx_bytes.saturating_add(self.rx_late_bytes)
    }

    pub const fn from_session_total(flow_id: u8, total: TransportEvidence) -> Self {
        Self {
            rx_maximum_silence_micros: total.rx_maximum_silence_micros,
            flow_id,
            rx_bytes: total.rx_bytes,
            tx_bytes: total.tx_bytes,
            rx_units: total.rx_units,
            tx_units: total.tx_units,
            rx_late_bytes: total.rx_late_bytes,
            rx_late_units: total.rx_late_units,
            elapsed_micros: total.elapsed_micros,
            transport_errors: total.transport_errors,
        }
    }

    pub const fn as_session_total(self) -> TransportEvidence {
        TransportEvidence {
            rx_maximum_silence_micros: self.rx_maximum_silence_micros,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
            rx_units: self.rx_units,
            tx_units: self.tx_units,
            rx_late_bytes: self.rx_late_bytes,
            rx_late_units: self.rx_late_units,
            elapsed_micros: self.elapsed_micros,
            transport_errors: self.transport_errors,
        }
    }
}

impl TransportEvidence {
    pub fn from_flows(flows: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY]) -> Self {
        // One missing flow observation must not masquerade as a complete
        // session value. Multi-flow users consume per-flow evidence instead.
        let mut active = flows.iter().flatten();
        let first = active
            .next()
            .and_then(|flow| flow.rx_maximum_silence_micros);
        let silence = if active.next().is_none() { first } else { None };
        flows.iter().flatten().copied().fold(
            Self {
                rx_maximum_silence_micros: silence,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_units: 0,
                tx_units: 0,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: 0,
                transport_errors: 0,
            },
            |mut total, flow| {
                total.rx_late_bytes = total.rx_late_bytes.saturating_add(flow.rx_late_bytes);
                total.rx_late_units = total.rx_late_units.saturating_add(flow.rx_late_units);
                total.rx_bytes = total.rx_bytes.saturating_add(flow.rx_bytes);
                total.tx_bytes = total.tx_bytes.saturating_add(flow.tx_bytes);
                total.rx_units = total.rx_units.saturating_add(flow.rx_units);
                total.tx_units = total.tx_units.saturating_add(flow.tx_units);
                total.elapsed_micros = total.elapsed_micros.max(flow.elapsed_micros);
                total.transport_errors =
                    total.transport_errors.saturating_add(flow.transport_errors);
                total
            },
        )
    }
}

/// Minimum typed radio evidence needed by ordinary UDP qualification. Richer
/// timing histograms remain diagnostic telemetry and never decide PASS/FAIL.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RadioEvidence {
    pub rx: Option<RxRadioEvidence>,
    pub tx: Option<TxRadioEvidence>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxRadioEvidence {
    /// ESP hardware RX format code observed for the measured interval.
    pub phy_format: u8,
    /// Complete benchmark-UDP HT40 observations at 800 ns GI.
    pub ht40_long_gi_frames: u32,
    /// Complete benchmark-UDP HT40 observations at 400 ns GI.
    pub ht40_short_gi_frames: u32,
    /// HT40 observations below MCS7. This is a subset of the two GI totals.
    pub ht40_below_mcs7_frames: u32,
    /// Benchmark vectors outside HT40 MCS0..7 geometry/format.
    pub ht_invalid_frames: u32,
    pub dma_buffer_full: u32,
    pub dma_fifo_overflow: u32,
    pub network_dropped: u32,
    pub irq_drain_saturated: u32,
    pub unhandled_irq_entries: u32,
    pub sequence_first: Option<u32>,
    pub sequence_highest: Option<u32>,
    pub sequence_gap_events: u32,
    pub sequence_forward_missing: u32,
    pub sequence_backward: u32,
    pub sequence_duplicates: u32,
    pub sequence_unsequenced: u32,
    pub s_mpdu_datagrams: u32,
    pub not_s_mpdu_datagrams: u32,
    pub s_mpdu_unavailable_datagrams: u32,
    pub s_mpdu_beacons: u32,
    pub not_s_mpdu_beacons: u32,
    pub s_mpdu_unavailable_beacons: u32,
    pub ampdu_datagrams: u32,
    pub not_ampdu_datagrams: u32,
    pub hardware_ampdu_datagrams: u32,
    pub hardware_not_ampdu_datagrams: u32,
    pub protocol_ampdu_datagrams: u32,
    pub protocol_not_ampdu_datagrams: u32,
    pub ampdu_unavailable_datagrams: u32,
    pub reorder_tid: u8,
    pub reorder_window: u16,
    pub reorder_first_samples: u32,
    pub reorder_first_tid: u8,
    pub reorder_first_start: u16,
    pub reorder_first_sequence: u16,
    pub reorder_first_distance: u16,
    pub reorder_current_occupied: u32,
    pub reorder_maximum_occupied: u32,
    pub rx_service_calls: u32,
    pub rx_frontier_histogram_samples: u32,
    pub mac_irq_entries: u32,
    pub mac_irq_classified_entries: u32,
}

/// Zero-copy RX accounting of one measured UDP RX session.
///
/// Published for every UDP RX session of an image whose network stack
/// adopts detached DMA slots, independently of driver observation.
///
/// Event counts cover the interval; `held_at_end` and `peak_held` are DMA
/// slots retained by the network stack, bounded by `cap`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxZeroCopyEvidence {
    /// Handoff slots the network may hold at once.
    pub cap: u16,
    /// Frames published to the stack without a copy.
    pub adopted: u32,
    /// Admissions refused at the cap, left to the copying path.
    pub copied_over_cap: u32,
    /// Admitted frames whose storage did not fit a packet and were copied.
    pub copied_unfit: u32,
    /// Admitted frames dropped because the queue was full or the link down.
    pub dropped: u32,
    pub held_at_end: u16,
    pub peak_held: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxRadioEvidence {
    /// Station-only terminal receipts within the snapshot interval. These do
    /// not include live or quarantined exchanges, nor prove UDP host delivery.
    pub station_terminal: StationTxTerminalEvidence,
    pub bandwidth_mhz: u16,
    pub aggregate_rate_kbps: u32,
    pub aggregates_prepared: u32,
    pub aggregate_publications: u32,
    /// Outstanding publications at the two coherent radio-executor snapshots.
    pub publications_pending_start: u32,
    pub publications_pending_end: u32,
    pub aggregates_completed: u32,
    pub subframes_prepared: u32,
    pub subframes_acknowledged: u32,
    pub individual_retries: u32,
    pub hardware_timeouts: u32,
    pub collisions: u32,
    pub minimum_subframes: u8,
    pub maximum_subframes: u8,
    pub prepared_histogram: [u32; 8],
    pub stopped_at_frame_limit: u32,
    pub stopped_at_capacity_limit: u32,
    pub stopped_on_empty_queue: u32,
    pub block_ack_samples: u32,
    /// Publications for which hardware reported a physically received
    /// BlockAck frame. This is independent of bitmap coverage: a received
    /// BlockAck may acknowledge zero subframes.
    pub block_ack_received: u32,
    pub success_without_block_ack: u32,
    pub nonzero_block_ack_control: u32,
    /// BlockAck-processing samples classified by acknowledged bitmap
    /// coverage. `full + partial + empty == block_ack_samples`; `empty` also
    /// includes publications for which no BlockAck frame was received.
    pub full_block_ack: u32,
    pub partial_block_ack: u32,
    pub empty_block_ack: u32,
    /// BlockAckReqs sent for aggregates whose protection exchange failed at
    /// every attempt, and those a BlockAck answered.
    pub block_ack_requests: u32,
    pub block_ack_requests_answered: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_publication_to_irq_samples: u32,
}

/// Timing and software-pipeline evidence for one bounded aggregate-TX interval.
///
/// This is separate from [`TxRadioEvidence`]: radio correctness and timing are
/// independently complete protocol records, and neither is reconstructed from
/// best-effort UART text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxAggregateTimingEvidence {
    pub preparation_micros: u32,
    pub preparation_max_micros: u32,
    pub publication_micros: u32,
    pub publication_max_micros: u32,
    pub exchange_micros: u32,
    pub exchange_max_micros: u32,
    pub first_exchanges: u32,
    pub first_exchange_micros: u32,
    pub first_exchange_max_micros: u32,
    pub retried_exchanges: u32,
    pub retry_publications: u32,
    pub retry_exchange_micros: u32,
    pub retry_exchange_max_micros: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_irq_service_micros: u32,
    pub tx_irq_service_max_micros: u32,
    pub tx_publication_to_irq_samples: u32,
    pub tx_publication_to_irq_micros: u32,
    pub tx_publication_to_irq_max_micros: u32,
    pub standby_prepared: u32,
    pub standby_published: u32,
    pub standby_cancelled: u32,
    /// Prepared owners retained across the measurement boundaries.
    pub standby_pending_start: u32,
    pub standby_pending_end: u32,
}

/// Sequence evidence collected at one finite UDP RX delivery stage.
///
/// Qualification traffic uses non-negative, non-wrapping `i32` sequence
/// numbers. Negative control markers are counted separately and never enter
/// the data-unit accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxSequenceStageEvidence {
    pub data_units: u32,
    pub first: Option<u32>,
    pub highest: Option<u32>,
    pub gap_events: u32,
    pub forward_missing: u32,
    pub late_recovered: u32,
    pub duplicates: u32,
    pub backward_unclassified: u32,
    pub first_anomaly: Option<u32>,
    pub control_markers: u32,
    pub data_after_terminal: u32,
}

/// Exact reconciliation of successful network admissions with UDP socket
/// consumption through a bounded qualification-only shadow ledger.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxConsumerLedgerEvidence {
    pub matched: u32,
    pub enqueued_not_consumed: u32,
    pub skipped_before_observed: u32,
    pub unexpected_consumer: u32,
    pub overflow: u32,
    pub first_expected: Option<u32>,
    pub first_observed: Option<u32>,
}

/// Correlation of application-level ordering defects with public QoS MAC
/// sequence/TID progression at the post-reorder frontier.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxMacOrderEvidence {
    /// First forward UDP gap with adjacent observations on the same QoS TID.
    /// MAC values are 12-bit sequence numbers, not an inferred loss count.
    pub first_forward_gap: Option<RxForwardGapEvidence>,
    pub backward_mac_backward: u32,
    pub backward_mac_same: u32,
    pub backward_mac_forward: u32,
    pub backward_mac_other_tid: u32,
    pub backward_mac_unavailable: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxForwardGapEvidence {
    pub previous_udp: u32,
    pub current_udp: u32,
    pub tid: u8,
    pub previous_mac: u16,
    pub current_mac: u16,
}

/// Reorder decisions relevant to delivery loss during one session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxReorderDeliveryEvidence {
    pub ingress: u32,
    pub ingress_retries: u32,
    pub direct: u32,
    pub buffered: u32,
    pub released: u32,
    pub missing: u32,
    pub stale: u32,
    pub gap_expiries: u32,
    pub maximum_occupied: u32,
    pub discarded: u32,
}

/// Complete typed evidence for the three UDP RX delivery frontiers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxDeliveryEvidence {
    pub post_reorder: RxSequenceStageEvidence,
    pub network_enqueued: RxSequenceStageEvidence,
    pub udp_consumer: RxSequenceStageEvidence,
    pub consumer_ledger: RxConsumerLedgerEvidence,
    pub mac_order: RxMacOrderEvidence,
    pub reorder: RxReorderDeliveryEvidence,
    pub network_queue_full: u32,
    pub network_invalid_length: u32,
    /// A receive packet owner could not be acquired.
    pub network_pool_exhausted: u32,
    /// The logical network endpoint was inactive at admission.
    pub network_link_down: u32,
}

/// Aggregate cooperative network scheduler evidence collected since boot.
///
/// Diagnostic images are cold-booted for each qualification cell, so this is
/// also the complete scheduler interval for that cell. No per-packet trace is
/// transported over the control link.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct NetworkSchedulerEvidence {
    pub polls: u32,
    pub ingress_calls: u32,
    pub ingress_packets: u32,
    pub egress_passes: u32,
    pub egress_tx_tokens: u32,
    pub egress_blocked: u32,
    pub ingress_budget_exhausted: u32,
    pub egress_budget_exhausted: u32,
    pub started_with_ingress: u32,
    pub started_with_egress: u32,
    pub exit_drained: u32,
    pub exit_work_budget: u32,
    pub exit_egress_credit: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ResultSummary {
    pub verdict: SessionVerdict,
    pub evidence_records: u16,
}

/// A traffic session's own verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum SessionVerdict {
    Passed,
    /// The first check that failed, in the order the verdict evaluates them,
    /// with its counts at the end of the session.
    Failed(SessionFailure),
}

impl SessionVerdict {
    pub const fn passed(self) -> bool {
        matches!(self, Self::Passed)
    }

    pub const fn failure(self) -> Option<SessionFailure> {
        match self {
            Self::Passed => None,
            Self::Failed(failure) => Some(failure),
        }
    }
}

impl core::fmt::Display for SessionVerdict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Passed => f.write_str("passed"),
            Self::Failed(failure) => write!(f, "failed: {failure}"),
        }
    }
}

/// Why a traffic session failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum SessionFailure {
    /// No datagram of the session arrived.
    NoDatagrams,
    /// The terminal marker never arrived.
    NoTerminal {
        received: u32,
        highest_sequence: u32,
    },
    ReceiveErrors(u32),
    SocketErrors(u32),
    TransmitErrors(u32),
    /// TCP connect or accept did not complete.
    NotConnected,
    /// TCP transfer units that did not finish.
    Incomplete {
        rx_units: u32,
        tx_units: u32,
    },
    /// Received TCP bytes did not match the stream pattern.
    PatternMismatch,
    /// Receive DMA or queue loss counted as a failure.
    Health {
        buffer_full: u32,
        fifo_overflow: u32,
        queue_dropped: u32,
    },
    /// The ownership epoch audit failed.
    OwnershipInvalid,
    /// The workload passed, but the control link lost or corrupted frames.
    ControlLink {
        cobs_errors: u32,
        checksum_errors: u32,
        decode_errors: u32,
        overflows: u32,
        tx_dropped: u32,
    },
}

impl core::fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::NoDatagrams => f.write_str("no datagram arrived"),
            Self::NoTerminal {
                received,
                highest_sequence,
            } => write!(
                f,
                "no terminal marker (received {received}, highest sequence {highest_sequence})"
            ),
            Self::ReceiveErrors(count) => write!(f, "{count} receive errors"),
            Self::SocketErrors(count) => write!(f, "{count} socket errors"),
            Self::TransmitErrors(count) => write!(f, "{count} transmit errors"),
            Self::NotConnected => f.write_str("TCP connection did not complete"),
            Self::Incomplete { rx_units, tx_units } => write!(
                f,
                "transfer incomplete (rx units {rx_units}, tx units {tx_units})"
            ),
            Self::PatternMismatch => f.write_str("received bytes did not match the pattern"),
            Self::Health {
                buffer_full,
                fifo_overflow,
                queue_dropped,
            } => write!(
                f,
                "receive loss (buffer full {buffer_full}, FIFO overflow {fifo_overflow}, \
                 queue dropped {queue_dropped})"
            ),
            Self::OwnershipInvalid => f.write_str("ownership epoch audit failed"),
            Self::ControlLink {
                cobs_errors,
                checksum_errors,
                decode_errors,
                overflows,
                tx_dropped,
            } => write!(
                f,
                "control link errors (COBS {cobs_errors}, checksum {checksum_errors}, decode \
                 {decode_errors}, overflows {overflows}, dropped {tx_dropped})"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Finished {
    pub summary: ResultSummary,
    pub evidence_crc32c: u32,
}

/// Logical station A-MPDU results after all aggregate and detached retries.
/// An unacknowledged MPDU may still have arrived when its ACK was lost.
/// Snapshots are not a drain barrier and exclude unfinished exchanges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationTxTerminalEvidence {
    pub exchanges: u32,
    pub mpdus: u32,
    pub acknowledged: u32,
    pub unacknowledged: u32,
    pub ordinary_recovered: u32,
    pub ordinary_failed: u32,
    pub invalid_statuses: u32,
}
