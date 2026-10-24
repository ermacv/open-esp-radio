//! The end of a station service that did not stop on command, and the
//! answer to role commands after it.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use crate::base::RejectReason;

use super::{ProbeStation, StopStation};

/// The station service ended without a stop command. It follows the link's
/// last `Disconnected` event and ends the station role of the boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationServiceEnded {
    pub epoch: u32,
    pub end: StationServiceEnd,
}

/// How the port was left after the fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationServiceEnd {
    /// The port is disabled and uninstalled.
    Failed(StationServiceFault),
    /// The disable after the fault failed too, with `disable`; the port
    /// fail-stopped.
    Poisoned {
        fault: StationServiceFault,
        disable: StationBackendFault,
    },
    /// Admission ended without another disable; the port fail-stopped.
    Quarantined(StationServiceFault),
}

/// The original failure that ended the station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationServiceFault {
    /// A management request exceeded its retained receive Block Ack responses.
    RxBlockAckCapacity,
    /// A receive Block Ack reply did not fit the control frame encoder.
    RxBlockAckEncode,
    /// The backend refused to enable the port on a channel.
    EnableRefused(StationBackendFault),
    Backend(StationBackendFault),
    Submission(StationBackendFault),
    Contract(StationContractViolation),
    /// The backend claimed it drained while `missing` was still outstanding.
    DrainedWithOutstanding {
        cause: StationBackendFault,
        missing: StationPendingTerminal,
    },
}

/// A backend returned owners that cannot belong to the submitted operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationContractViolation {
    UnexpectedAggregate,
    UnexpectedTerminal(StationPendingTerminal),
    TerminalAfterEventEnd,
    UnexpectedAccessPointTerminal,
    UnexpectedTsfSample,
    GroupTerminalWithoutRequest,
    GroupSlotMismatch,
    GroupTerminalKindMismatch,
    TxContextBufferMismatch,
    StationKeyBank(StationKeyContractViolation),
}

/// A key resource returned against the station's key bank contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationKeyContractViolation {
    IncompleteBank,
    MissingAssociation,
}

/// A terminal the service still expected from the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPendingTerminal {
    /// The TX ticket of this index.
    Transmit(u8),
    Control(StationControlTerminal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationControlTerminal {
    Configure,
    PairwiseRemoval,
    GroupChange,
    GateClosed,
}

/// The ESP32-S31 backend's fault, down to the kind of its lower-MAC or
/// receive cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationBackendFault {
    Core(StationLowerMacFault),
    Receive(StationReceiveFault),
    Deadline,
    Capacity,
    Clock,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLowerMacFault {
    Tx,
    RxBlockAckReadback,
    Key,
    RxBlockAck,
    NoRxBlockAckBank,
    Abandoned,
    KeyUseMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationReceiveFault {
    StopRequested,
    Prepare,
    LineRefused,
    Start,
    Service,
    ForeignExecutor,
}

/// The station role as a role command finds it. It is not a wire type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationRole {
    /// No station was started.
    Idle,
    Running,
    /// A stop was accepted and has not completed.
    Stopping,
    /// The service ended: `StationServiceEnded` or `StationStopped` was
    /// published. Nothing would answer a later command.
    Ended,
}

impl StopStation {
    /// The refusal of a stop in `role`, before its admission to a full
    /// queue (`Busy`). Only a running station accepts a stop, so every
    /// stop is answered.
    pub const fn refusal(role: StationRole) -> Option<RejectReason> {
        match role {
            StationRole::Running => None,
            StationRole::Idle | StationRole::Stopping | StationRole::Ended => {
                Some(RejectReason::InvalidState)
            }
        }
    }
}

impl ProbeStation {
    /// The refusal of this probe in `role`, before its admission to a full
    /// queue (`Busy`).
    pub const fn refusal(self, role: StationRole) -> Option<RejectReason> {
        match role {
            StationRole::Running if self.valid() => None,
            StationRole::Running => Some(RejectReason::InvalidConfiguration),
            StationRole::Idle | StationRole::Stopping | StationRole::Ended => {
                Some(RejectReason::InvalidState)
            }
        }
    }
}
