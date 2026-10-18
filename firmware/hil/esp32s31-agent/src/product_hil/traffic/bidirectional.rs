#![forbid(unsafe_code)]

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_hil_protocol::network::SessionVerdict;
use oer_hil_protocol::{
    network::CpuLoadEvidence, network::Direction, network::FlowTransportEvidence,
    network::RadioEvidence, network::RxDeliveryEvidence, network::RxZeroCopyEvidence,
    network::SESSION_FLOW_CAPACITY, network::TxAggregateTimingEvidence,
};

use crate::console::{ActiveSession, complete_session};

use super::SessionChannel;

pub(in crate::product_hil) type BidirectionalSessionChannel =
    Channel<CriticalSectionRawMutex, ActiveSession, 1>;
pub(in crate::product_hil) type BidirectionalResultChannel =
    Channel<CriticalSectionRawMutex, OpenRadioBidirectionalResult, 2>;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::product_hil) enum OpenRadioBidirectionalDirection {
    Rx,
    Tx,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct OpenRadioBidirectionalResult {
    session_id: u64,
    direction: OpenRadioBidirectionalDirection,
    flow_evidence: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    radio: Option<RadioEvidence>,
    tx_timing: Option<TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    rx_zero_copy: Option<RxZeroCopyEvidence>,
    cpu_load: Option<CpuLoadEvidence>,
    verdict: SessionVerdict,
}

impl OpenRadioBidirectionalResult {
    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per recorded evidence field of the result"
    )]
    pub(in crate::product_hil) const fn new(
        session_id: u64,
        direction: OpenRadioBidirectionalDirection,
        flow_evidence: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
        radio: Option<RadioEvidence>,
        tx_timing: Option<TxAggregateTimingEvidence>,
        rx_delivery: Option<RxDeliveryEvidence>,
        rx_zero_copy: Option<RxZeroCopyEvidence>,
        cpu_load: Option<CpuLoadEvidence>,
        verdict: SessionVerdict,
    ) -> Self {
        Self {
            session_id,
            direction,
            flow_evidence,
            radio,
            tx_timing,
            rx_delivery,
            rx_zero_copy,
            cpu_load,
            verdict,
        }
    }
}

pub(in crate::product_hil) async fn run_open_radio_bidirectional_session_coordinator(
    input: &'static SessionChannel,
    rx_sessions: &'static BidirectionalSessionChannel,
    tx_sessions: &'static BidirectionalSessionChannel,
    results: &'static BidirectionalResultChannel,
) -> ! {
    loop {
        let session = input.receive().await;
        match session.config.direction {
            Direction::Rx => {
                rx_sessions.send(session).await;
                complete_single_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Rx,
                    results.receive().await,
                )
                .await;
            }
            Direction::Tx => {
                tx_sessions.send(session).await;
                complete_single_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Tx,
                    results.receive().await,
                )
                .await;
            }
            Direction::Bidirectional => {
                rx_sessions.send(session).await;
                tx_sessions.send(session).await;
                let first = results.receive().await;
                let second = results.receive().await;
                // This coordinator hands each direction its one session and
                // collects both results before the next session starts.
                assert!(
                    first.session_id == session.session_id
                        && second.session_id == session.session_id
                        && first.direction != second.direction,
                    "bidirectional results belong to the coordinated session",
                );
                let flow_evidence = merge_flow_evidence(first.flow_evidence, second.flow_evidence);
                complete_session(
                    session.session_id,
                    flow_evidence,
                    merge_radio(first.radio, second.radio),
                    first.tx_timing.or(second.tx_timing),
                    if first.direction == OpenRadioBidirectionalDirection::Rx {
                        first.rx_delivery
                    } else {
                        second.rx_delivery
                    },
                    first.rx_zero_copy.or(second.rx_zero_copy),
                    // Each direction measured its own window; neither is
                    // the session's.
                    None,
                    match first.verdict {
                        SessionVerdict::Passed => second.verdict,
                        failed => failed,
                    },
                )
                .await;
                super::session_evidence_published();
            }
        }
    }
}

async fn complete_single_direction(
    session_id: u64,
    expected_direction: OpenRadioBidirectionalDirection,
    result: OpenRadioBidirectionalResult,
) {
    assert!(
        result.session_id == session_id && result.direction == expected_direction,
        "a single-direction result belongs to the coordinated session",
    );
    complete_session(
        session_id,
        result.flow_evidence,
        result.radio,
        result.tx_timing,
        result.rx_delivery,
        result.rx_zero_copy,
        result.cpu_load,
        result.verdict,
    )
    .await;
    super::session_evidence_published();
}

fn merge_flow_evidence(
    first: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    second: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
) -> [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY] {
    core::array::from_fn(|index| match (first[index], second[index]) {
        (Some(first), Some(second)) if first.flow_id == second.flow_id => {
            Some(FlowTransportEvidence {
                rx_maximum_silence_micros: first
                    .rx_maximum_silence_micros
                    .or(second.rx_maximum_silence_micros),
                flow_id: first.flow_id,
                rx_bytes: first.rx_bytes.saturating_add(second.rx_bytes),
                tx_bytes: first.tx_bytes.saturating_add(second.tx_bytes),
                rx_units: first.rx_units.saturating_add(second.rx_units),
                tx_units: first.tx_units.saturating_add(second.tx_units),
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: first.elapsed_micros.max(second.elapsed_micros),
                transport_errors: first
                    .transport_errors
                    .saturating_add(second.transport_errors),
            })
        }
        (None, None) => None,
        _ => panic!("both directions of one session report the same flows"),
    })
}

pub(in crate::product_hil) async fn complete_open_radio_bidirectional_direction(
    results: &'static BidirectionalResultChannel,
    result: OpenRadioBidirectionalResult,
) {
    results.send(result).await;
}

fn merge_radio(
    first: Option<RadioEvidence>,
    second: Option<RadioEvidence>,
) -> Option<RadioEvidence> {
    match (first, second) {
        (None, None) => None,
        (first, second) => Some(RadioEvidence {
            rx: first
                .and_then(|value| value.rx)
                .or_else(|| second.and_then(|value| value.rx)),
            tx: first
                .and_then(|value| value.tx)
                .or_else(|| second.and_then(|value| value.tx)),
        }),
    }
}
