//! The target's network traffic sessions: configuration, arming and start
//! of a session, its readiness, the typed evidence it publishes when it
//! finishes, and the replay that proves the target retained that evidence
//! before the host acknowledges it.

use std::time::{Duration, Instant};

use oer_hil_link::{Received, Result, SerialCapture, error::LinkError};
use oer_hil_protocol::{
    Endpoint, Message,
    base::LinkHealth,
    network::{
        Direction, EvidenceRecord, Finished, FlowTransportEvidence, NetworkSchedulerEvidence,
        RadioEvidence, RxDeliveryEvidence, RxZeroCopyEvidence, SESSION_FLOW_CAPACITY,
        SessionConfig, SessionLinkRequirements, SessionReady, TransportEvidence,
        TxAggregateTimingEvidence, evidence_crc32c,
    },
    system::StackUsage,
};

use crate::validation::validate_stack_usage;

/// How long each readiness step of a session may take.
const PROTOCOL_READY_TIMEOUT: Duration = oer_hil_link::PROTOCOL_READY_TIMEOUT;

/// Configure, Arm, Start and up to two directional SessionReady waits. The
/// collector runs before these operations and must cover their failure bounds.
pub const SESSION_START_TIMEOUT: Duration = PROTOCOL_READY_TIMEOUT.saturating_mul(5);

/// One started session of a capture.
#[derive(Clone, Copy, Debug)]
pub struct SessionHandle {
    pub(crate) session_id: u64,
    pub(crate) first_event: usize,
    pub(crate) flow_ids: [Option<u8>; SESSION_FLOW_CAPACITY],
}

/// The typed evidence a finished session published, checked complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct SessionEvidence {
    pub transport: TransportEvidence,
    pub flow_transport: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    pub radio: Option<RadioEvidence>,
    pub tx_timing: Option<TxAggregateTimingEvidence>,
    pub rx_delivery: Option<RxDeliveryEvidence>,
    pub network_scheduler: Option<NetworkSchedulerEvidence>,
    pub rx_zero_copy: Option<RxZeroCopyEvidence>,
    pub stack: StackUsage,
    pub link: LinkHealth,
    pub finished: Finished,
}

/// The network sessions of a capture of the board under test.
pub trait NetworkSession {
    /// Configure, arm and start a session; returns once the target reports
    /// every configured direction ready.
    fn start_session(&self, config: SessionConfig) -> Result<SessionHandle>;

    /// Reserve one exact current-boot/session identity before Configure so
    /// both UDP payload directions can be correlated to this lifecycle stage.
    fn start_identified_udp_session(
        &self,
        build: impl FnOnce(oer_hil_protocol::network::UdpSessionPayloadIdentity) -> SessionConfig,
    ) -> Result<(
        SessionHandle,
        oer_hil_protocol::network::UdpSessionPayloadIdentity,
    )>;

    /// The complete typed evidence of `session`, within `timeout`.
    fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence>;

    /// Verify that the target retained the complete immutable result before
    /// authorizing its removal. This happens after the measured traffic.
    fn acknowledge_session(&self, session: SessionHandle) -> Result<()>;
}

impl NetworkSession for SerialCapture {
    fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        start_session_with_id(self, self.next_session_id(), config)
    }

    fn start_identified_udp_session(
        &self,
        build: impl FnOnce(oer_hil_protocol::network::UdpSessionPayloadIdentity) -> SessionConfig,
    ) -> Result<(
        SessionHandle,
        oer_hil_protocol::network::UdpSessionPayloadIdentity,
    )> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("device omitted the current boot identity")?;
        let session_id = self.next_session_id();
        let identity = oer_hil_protocol::network::UdpSessionPayloadIdentity::new(
            boot_id.rotate_left(32).wrapping_add(session_id),
        );
        let session = start_session_with_id(self, session_id, build(identity))?;
        Ok((session, identity))
    }

    fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        wait_for_session(self, session, timeout)
    }

    fn acknowledge_session(&self, session: SessionHandle) -> Result<()> {
        let original = wait_for_session(self, session, Duration::ZERO)?;
        let first_event = self.event_cursor();
        if let Err(reason) = self.call(
            session.session_id,
            oer_hil_protocol::network::ReplayResult,
            PROTOCOL_READY_TIMEOUT,
        )? {
            return Err(LinkError::protocol(format!(
                "target cannot replay completed session {}: {reason:?}",
                session.session_id
            ))
            .into());
        }
        let replay = wait_for_session(
            self,
            SessionHandle {
                first_event,
                ..session
            },
            PROTOCOL_READY_TIMEOUT,
        )?;
        if replay != original {
            return Err(LinkError::protocol(format!(
                "target changed the retained result for session {}",
                session.session_id
            ))
            .into());
        }
        expect_accepted(
            self,
            session.session_id,
            oer_hil_protocol::network::AcknowledgeResult,
            "acknowledgement",
        )
    }
}

/// Whether a reported readiness covers the `expected` direction of a session
/// configured for `configured`, with its link requirements.
pub(crate) fn session_ready_covers(
    configured: Direction,
    reported: SessionReady,
    expected: Direction,
    requirements: SessionLinkRequirements,
) -> bool {
    let direction_covers = reported.direction == expected
        || (configured == Direction::Bidirectional
            && reported.direction == Direction::Bidirectional);
    let requirements_met =
        expected != Direction::Tx || reported.tx_block_ack_tid == requirements.tx_block_ack_tid;
    direction_covers && requirements_met
}

fn expect_accepted<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
    capture: &SerialCapture,
    session_id: u64,
    body: E,
    operation: &str,
) -> Result<()> {
    capture
        .call(session_id, body, PROTOCOL_READY_TIMEOUT)
        .map_err(|error| {
            oer_hil_link::error::context(format!("session {operation} command failed"), error)
        })?
        .map(|_| ())
        .map_err(|reason| format!("device rejected session {operation}: {reason:?}").into())
}

fn start_session_with_id(
    capture: &SerialCapture,
    session_id: u64,
    config: SessionConfig,
) -> Result<SessionHandle> {
    let first_event = capture.event_cursor();
    let direction = config.direction;
    let link_requirements = config.link_requirements;
    let flow_ids = config.flows.map(|flow| flow.map(|flow| flow.flow_id));
    let handle = SessionHandle {
        session_id,
        first_event,
        flow_ids,
    };
    expect_accepted(
        capture,
        session_id,
        oer_hil_protocol::network::Configure(config),
        "configuration",
    )?;
    expect_accepted(capture, session_id, oer_hil_protocol::network::Arm, "arm")?;
    expect_accepted(
        capture,
        session_id,
        oer_hil_protocol::network::Start,
        "start",
    )?;
    let expected_directions: &[Direction] = match direction {
        Direction::Rx => &[Direction::Rx],
        Direction::Tx => &[Direction::Tx],
        Direction::Bidirectional => &[Direction::Rx, Direction::Tx],
    };
    for expected in expected_directions {
        wait_for_session_event(
            capture,
            handle,
            PROTOCOL_READY_TIMEOUT,
            |_, reported: &oer_hil_protocol::network::SessionReady| {
                session_ready_covers(direction, *reported, *expected, link_requirements)
            },
        )?
        .ok_or_else(|| {
            format!(
                "device did not publish {expected:?} data-plane readiness for session \
                 {session_id}; required TX BlockAck TID: {:?}",
                link_requirements.tx_block_ack_tid,
            )
        })?;
    }
    Ok(handle)
}

/// The first `M` of `session` that `accept` takes; the session's failure
/// ends the wait as an error.
fn wait_for_session_event<M: Message>(
    capture: &SerialCapture,
    session: SessionHandle,
    timeout: Duration,
    accept: impl Fn(&Received, &M) -> bool,
) -> Result<Option<(Received, M)>> {
    let found = capture.wait_for_message_after(session.first_event, timeout, |message| {
        message.session_id == session.session_id
            && (message
                .decode::<M>()
                .is_some_and(|body| accept(message, &body))
                || message.is::<oer_hil_protocol::network::Failed>())
    })?;
    let Some(message) = found else {
        return Ok(None);
    };
    if let Some(oer_hil_protocol::network::Failed(reason)) = message.decode() {
        return Err(format!("target session {} failed: {reason:?}", session.session_id).into());
    }
    let body = message.decode::<M>().expect("the accepted message decodes");
    Ok(Some((message, body)))
}

/// The first evidence record of `session` that `pick` takes.
fn session_evidence<T>(
    capture: &SerialCapture,
    session: SessionHandle,
    timeout: Duration,
    pick: impl Fn(EvidenceRecord) -> Option<T>,
) -> Result<Option<T>> {
    Ok(wait_for_session_event(
        capture,
        session,
        timeout,
        |_, oer_hil_protocol::network::Evidence(record)| pick(*record).is_some(),
    )?
    .and_then(|(_, oer_hil_protocol::network::Evidence(record))| pick(record)))
}

fn wait_for_session(
    capture: &SerialCapture,
    session: SessionHandle,
    timeout: Duration,
) -> Result<SessionEvidence> {
    let deadline = oer_hil_link::transport::events::deadline_after(timeout);
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let transport = session_evidence(capture, session, remaining(), |record| match record {
        EvidenceRecord::Transport(transport) => Some(transport),
        _ => None,
    })?
    .ok_or("device did not publish structured session evidence")?;
    let mut flow_transport = [None; SESSION_FLOW_CAPACITY];
    for (index, flow_id) in session.flow_ids.into_iter().enumerate() {
        let Some(flow_id) = flow_id else {
            continue;
        };
        flow_transport[index] = Some(
            session_evidence(capture, session, remaining(), |record| match record {
                EvidenceRecord::FlowTransport(flow) if flow.flow_id == flow_id => Some(flow),
                _ => None,
            })?
            .ok_or_else(|| {
                format!("device did not publish transport evidence for flow {flow_id}")
            })?,
        );
    }
    let flow_total = TransportEvidence::from_flows(flow_transport);
    if flow_total != transport {
        return Err(format!(
            "per-flow transport total does not match session aggregate: flows={flow_total:?} aggregate={transport:?}"
        )
        .into());
    }
    let link = session_evidence(capture, session, remaining(), |record| match record {
        EvidenceRecord::Link(link) => Some(link),
        _ => None,
    })?
    .ok_or("device did not publish structured protocol-link evidence")?;
    if link.rx_cobs_errors != 0
        || link.rx_checksum_errors != 0
        || link.rx_decode_errors != 0
        || link.rx_overflows != 0
        || link.tx_dropped != 0
    {
        return Err(
            LinkError::protocol(format!("device protocol link is unhealthy: {link:?}")).into(),
        );
    }
    let stack = session_evidence(capture, session, remaining(), |record| match record {
        EvidenceRecord::Stack(stack) => Some(stack),
        _ => None,
    })?
    .ok_or("device did not publish structured stack evidence")?;
    validate_stack_usage(stack)?;
    let (_, finished) = wait_for_session_event(
        capture,
        session,
        remaining(),
        |_, _: &oer_hil_protocol::network::Finished| true,
    )?
    .ok_or("device did not finish the structured HIL session")?;
    let radio = session_evidence(capture, session, Duration::ZERO, |record| match record {
        EvidenceRecord::Radio(radio) => Some(radio),
        _ => None,
    })?;
    let tx_timing = session_evidence(capture, session, Duration::ZERO, |record| match record {
        EvidenceRecord::TxAggregateTiming(timing) => Some(timing),
        _ => None,
    })?;
    let rx_delivery = session_evidence(capture, session, Duration::ZERO, |record| match record {
        EvidenceRecord::RxDelivery(delivery) => Some(delivery),
        _ => None,
    })?;
    let network_scheduler =
        session_evidence(capture, session, Duration::ZERO, |record| match record {
            EvidenceRecord::NetworkScheduler(evidence) => Some(evidence),
            _ => None,
        })?;
    let rx_zero_copy = session_evidence(capture, session, Duration::ZERO, |record| match record {
        EvidenceRecord::RxZeroCopy(evidence) => Some(evidence),
        _ => None,
    })?;
    if let Some(zero_copy) = rx_zero_copy {
        crate::validation::validate_rx_zero_copy(zero_copy)?;
    }
    let expected_records = 3
        + u16::try_from(flow_transport.iter().flatten().count())?
        + u16::from(radio.is_some())
        + u16::from(tx_timing.is_some())
        + u16::from(rx_delivery.is_some())
        + u16::from(network_scheduler.is_some())
        + u16::from(rx_zero_copy.is_some());
    if finished.summary.evidence_records != expected_records {
        return Err(format!(
            "device reported {} evidence records but published {expected_records} typed records",
            finished.summary.evidence_records
        )
        .into());
    }
    let mut records = Vec::with_capacity(usize::from(finished.summary.evidence_records));
    records.push(EvidenceRecord::Transport(transport));
    for flow in flow_transport.iter().flatten().copied() {
        records.push(EvidenceRecord::FlowTransport(flow));
    }
    if let Some(radio) = radio {
        records.push(EvidenceRecord::Radio(radio));
    }
    if let Some(timing) = tx_timing {
        records.push(EvidenceRecord::TxAggregateTiming(timing));
    }
    if let Some(delivery) = rx_delivery {
        records.push(EvidenceRecord::RxDelivery(delivery));
    }
    if let Some(scheduler) = network_scheduler {
        records.push(EvidenceRecord::NetworkScheduler(scheduler));
    }
    if let Some(zero_copy) = rx_zero_copy {
        records.push(EvidenceRecord::RxZeroCopy(zero_copy));
    }
    records.push(EvidenceRecord::Link(link));
    records.push(EvidenceRecord::Stack(stack));
    let checksum = evidence_crc32c(&records)
        .map_err(|error| format!("cannot checksum structured HIL evidence: {error}"))?;
    if checksum != finished.evidence_crc32c {
        return Err(format!(
            "structured HIL evidence checksum mismatch: host={checksum:#010x} device={:#010x}",
            finished.evidence_crc32c
        )
        .into());
    }
    let session_evidence = SessionEvidence {
        transport,
        flow_transport,
        radio,
        tx_timing,
        rx_delivery,
        network_scheduler,
        rx_zero_copy,
        stack,
        link,
        finished,
    };
    if session_evidence.flow_transport.iter().flatten().count()
        != session.flow_ids.iter().flatten().count()
    {
        return Err("structured session lost configured flow evidence".into());
    }
    Ok(session_evidence)
}
