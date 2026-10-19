//! A network owner's state, its failures and its evidence records.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[cfg(feature = "network")]
use super::*;
#[cfg(feature = "network")]
use crate::base::LinkHealth;
#[cfg(feature = "network")]
use crate::system::StackUsage;

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
#[cfg(feature = "network")]
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

/// Computes the digest carried by [`crate::network::Finished`] for an ordered evidence
/// set.
///
/// The digest covers the canonical postcard representation, including the
/// slice length. Both the target and host use this helper so missing, reordered
/// or mismatched evidence cannot satisfy a `Finished` event. Session identity
/// is checked separately from the surrounding [`crate::Envelope`].
#[cfg(feature = "network")]
pub fn evidence_crc32c(evidence: &[EvidenceRecord]) -> Result<u32, crate::EncodeError> {
    let mut encoded = [0_u8; crate::MAX_POSTCARD_BYTES];
    let payload =
        postcard::to_slice(evidence, &mut encoded).map_err(|_| crate::EncodeError::Serialize)?;
    Ok(crate::framing::crc32c(payload))
}
