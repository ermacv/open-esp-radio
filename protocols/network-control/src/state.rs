//! A network owner's state, its failures and its evidence records.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use super::*;
use oer_hil_protocol::base::LinkHealth;
use oer_hil_protocol::system::StackUsage;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum SessionState {
    Booting,
    WaitingForInitialization,
    Idle,
    Configured,
    Armed,
    Running,
    Draining,
    Finished,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StateChange {
    pub previous: SessionState,
    pub current: SessionState,
}

/// Query result used to recover after an uncertain UART response without
/// guessing whether a session was configured, started or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OperationStatus {
    pub state: SessionState,
    pub configured_session_id: Option<u64>,
    pub completed_session_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum FailureCode {
    Configuration,
    Network,
    Transport,
    Timeout,
    EvidenceOverflow,
    Internal,
}

/// One record of a session's evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum EvidenceRecord {
    Transport(TransportEvidence),
    FlowTransport(FlowTransportEvidence),
    Radio(RadioEvidence),
    TxAggregateTiming(TxAggregateTimingEvidence),
    RxDelivery(RxDeliveryEvidence),
    NetworkScheduler(NetworkSchedulerEvidence),
    Link(LinkHealth),
    Stack(StackUsage),
    RxZeroCopy(RxZeroCopyEvidence),
}

/// Computes the digest carried by [`crate::Finished`] for an ordered evidence
/// set.
///
/// The digest covers the canonical postcard representation, including the
/// slice length. Both the target and host use this helper so missing, reordered
/// or mismatched evidence cannot satisfy a `Finished` event. Session identity
/// is checked separately from the surrounding [`oer_hil_protocol::Envelope`].
pub fn evidence_crc32c(evidence: &[EvidenceRecord]) -> Result<u32, oer_hil_protocol::EncodeError> {
    let mut encoded = [0_u8; oer_hil_protocol::MAX_POSTCARD_BYTES];
    let payload = postcard::to_slice(evidence, &mut encoded)
        .map_err(|_| oer_hil_protocol::EncodeError::Serialize)?;
    Ok(crc::Crc::<u32>::new(&crc::CRC_32_ISCSI).checksum(payload))
}
