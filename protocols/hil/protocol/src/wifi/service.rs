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
    /// The port poisoned during an owner transition (enable or disable);
    /// it fail-stopped.
    Poisoned(StationServiceFault),
    /// Admission ended without another disable; the port fail-stopped.
    Quarantined(StationServiceFault),
    /// The radio front end never registered the station port, so no port
    /// was installed and no service ran.
    Unavailable,
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
    /// The radio front end did not retune the stopped port.
    Retune(StationRetuneFault),
    /// The device lost its registers to an earlier failure.
    Abandoned,
}

/// Why the radio front end did not retune the stopped port.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationRetuneFault {
    /// The MAC was still active in `state`; nothing was posted.
    MacActive { state: u8 },
    /// The RX walker was still enabled; nothing was posted.
    RxWalkerEnabled,
    /// The radio system refused before any hardware write.
    Refused,
    /// The client's own hardware work failed and was rolled back.
    ClientFailed,
    /// The PHY domain is poisoned by the first failure it names.
    Poisoned(StationRadioPoison),
}

/// The first failure that poisoned the shared PHY domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationRadioPoison {
    pub origin: StationRadioOperation,
    pub cause: StationRadioPoisonCause,
}

/// The radio system's operation that saw the failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationRadioOperation {
    /// The periodic PHY tracking tick between requests.
    TrackingTick,
    /// Registration or RF wake before a join.
    Prepare,
    /// The RF close after the last member left.
    CloseRf,
    Request {
        client: StationRadioClient,
        request: StationRadioRequest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationRadioClient {
    Wifi,
    Bluetooth,
    Ieee802154,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationRadioRequest {
    Join,
    Track,
    Retune,
    Leave,
}

/// The step that poisoned the shared PHY domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationRadioPoisonCause {
    /// Tracking was refused before register access: its clock or the
    /// domain's state.
    TrackingRefused(StationPhyDomainError),
    Tracking(StationPhyFault),
    Channel(StationPhyFault),
    Rf(StationPhyFault),
    Registration(StationPhyFault),
    Domain(StationPhyDomainError),
    CommonPower,
    ModemClock,
    Rollback,
    Controller,
    Custody,
}

/// How a started PHY transaction failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPhyFault {
    EdgeTimedOut,
    CapabilityUnavailable,
    HardwareInvariant,
    RfOperationLimit,
    UnexpectedBinding,
    RegistrationEpochMismatch,
    DeadlineOverflow,
    Sequence,
    Radio,
}

/// Why the PHY domain refused a step before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPhyDomainError {
    NotRegistered,
    AlreadyRegistered,
    TrackingPending,
    NoTrackingPending,
    Poisoned,
    RfClosed,
    RfOpen,
    EpochMismatch,
    ClientsActive,
    Clock,
    ClientAbsent,
    /// The tracking clock's deadline did not fit.
    TimeOverflow,
    /// The tracking clock ran backwards from `previous_us` to `now_us`.
    TimeReversed {
        previous_us: u64,
        now_us: u64,
    },
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
