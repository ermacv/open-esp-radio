//! The variant paths an abandoned station's record keeps: why the port
//! ended the body loan, then the port's first fault. A subtree the path
//! does not spell out marks the path cut (`DiagPath::truncated`).
use super::*;
use oer_portable_diagnostics::{DiagPath, DiagnosticPath};

impl DiagnosticPath for Abandoned {
    fn push_path(&self, path: &mut DiagPath) {
        self.cause.push_path(path);
        if let Some(fault) = &self.fault {
            fault.push_path(path);
        }
    }
}

impl DiagnosticPath for AbandonCause {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::QueuesUnprepared => path.name("QueuesUnprepared"),
            Self::MissingResource(resource) => {
                path.name("MissingResource");
                path.name(*resource);
            }
            Self::BodyAlreadyIssued => path.name("BodyAlreadyIssued"),
            Self::Lifecycle(cause) => {
                path.name("Lifecycle");
                cause.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for RetirementCause {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Cancelled => path.name("Cancelled"),
            Self::FailStopRequested => path.name("FailStopRequested"),
            Self::Construction => path.name("Construction"),
            Self::BodyAlreadyIssued => path.name("BodyAlreadyIssued"),
            Self::Deadline => path.name("Deadline"),
            Self::StopUnconfirmed => path.name("StopUnconfirmed"),
            Self::NetworkDrain(drain) => {
                path.name("NetworkDrain");
                path.name(match drain.cause {
                    NetworkDrainCause::Deadline => "Deadline",
                    NetworkDrainCause::ParticipantLost => "ParticipantLost",
                });
                path.word(u32::try_from(drain.held).unwrap_or(u32::MAX));
            }
            Self::InvalidState(observed) => {
                path.name("InvalidState");
                path.name(match observed.storage {
                    SlotStorage::Empty => "Empty",
                    SlotStorage::Registered => "Registered",
                    SlotStorage::Vacant => "Vacant",
                    SlotStorage::Unavailable => "Unavailable",
                });
                path.word(u32::from(observed.phase));
                path.word(u32::from(observed.invocation_present));
            }
        }
    }
}

impl DiagnosticPath for BackendFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Core(fault) => {
                path.name("Core");
                fault.push_path(path);
            }
            Self::Receive(fault) => {
                path.name("Receive");
                path.name(match fault {
                    ReceiveFault::StopRequested => "StopRequested",
                    ReceiveFault::Prepare(_) => "Prepare",
                    ReceiveFault::LineRefused => "LineRefused",
                    ReceiveFault::Start(_) => "Start",
                    ReceiveFault::Service(_) => "Service",
                    ReceiveFault::ForeignExecutor => "ForeignExecutor",
                });
                if matches!(
                    fault,
                    ReceiveFault::Prepare(_) | ReceiveFault::Start(_) | ReceiveFault::Service(_)
                ) {
                    path.truncated();
                }
            }
            Self::Deadline => path.name("Deadline"),
            Self::Capacity => path.name("Capacity"),
            Self::Clock => path.name("Clock"),
            Self::Retune(error) => {
                path.name("Retune");
                path.name(match error {
                    RetuneError::NotStopped(_) => "NotStopped",
                    RetuneError::Radio(_) => "Radio",
                });
                path.truncated();
            }
            Self::Abandoned => path.name("Abandoned"),
            Self::Arena(fault) => {
                path.name("Arena");
                match fault {
                    ArenaFault::Chain(error) => {
                        path.name("Chain");
                        error.push_path(path);
                    }
                    ArenaFault::Core(fault) => {
                        path.name("Core");
                        fault.push_path(path);
                    }
                    ArenaFault::State => path.name("State"),
                    ArenaFault::Watchdog => path.name("Watchdog"),
                }
            }
            Self::TuneNotRecorded => path.name("TuneNotRecorded"),
            Self::Setting(fault) => {
                path.name("Setting");
                path.name(match fault {
                    SettingFault::UnsupportedChannel => "UnsupportedChannel",
                    SettingFault::ChannelUnset => "ChannelUnset",
                    SettingFault::EdcaLifetime => "EdcaLifetime",
                    SettingFault::BlockAckSequence => "BlockAckSequence",
                });
            }
            Self::HardwareUnavailable => path.name("HardwareUnavailable"),
            Self::CoreViewUnavailable(op) => {
                path.name("CoreViewUnavailable");
                path.name(match op {
                    CoreOperation::Command => "Command",
                    CoreOperation::Tbtt => "Tbtt",
                    CoreOperation::Arena => "Arena",
                    CoreOperation::Publish => "Publish",
                });
            }
            Self::DuplicateCompletion { queue } => queue_fault(path, "DuplicateCompletion", *queue),
            Self::RemovalOverflow => path.name("RemovalOverflow"),
            Self::CompletionWithoutPending { queue } => {
                queue_fault(path, "CompletionWithoutPending", *queue);
            }
            Self::DescriptorMissing { queue } => queue_fault(path, "DescriptorMissing", *queue),
            Self::ReturnRefused { queue } => queue_fault(path, "ReturnRefused", *queue),
            Self::BlockAckReportMissing { queue } => {
                queue_fault(path, "BlockAckReportMissing", *queue);
            }
            Self::AirMissing { queue } => queue_fault(path, "AirMissing", *queue),
            Self::AttemptCounter { queue } => queue_fault(path, "AttemptCounter", *queue),
            Self::StageMissing { queue } => queue_fault(path, "StageMissing", *queue),
            Self::QueueFull { queue } => queue_fault(path, "QueueFull", *queue),
            Self::ReceiveOverflow => path.name("ReceiveOverflow"),
            Self::ReceiveRange => path.name("ReceiveRange"),
            Self::EventQueueFull => path.name("EventQueueFull"),
            Self::ArenaCoreContract => path.name("ArenaCoreContract"),
            Self::InstallationResources => path.name("InstallationResources"),
            Self::EdcaUnsupported => path.name("EdcaUnsupported"),
            Self::ChannelNotPermitted => path.name("ChannelNotPermitted"),
            Self::KeySlotMissing(op) => {
                path.name("KeySlotMissing");
                path.name(match op {
                    KeyOperation::Install => "Install",
                    KeyOperation::Remove => "Remove",
                    KeyOperation::Replace => "Replace",
                    KeyOperation::Select => "Select",
                    KeyOperation::Transmit => "Transmit",
                });
            }
            Self::ReceiveAccounting => path.name("ReceiveAccounting"),
            Self::CommandExpired(kind) => {
                path.name("CommandExpired");
                path.name(match kind {
                    PortCommandKind::InstallPairwise => "InstallPairwise",
                    PortCommandKind::InstallGroup => "InstallGroup",
                    PortCommandKind::ReplaceGroup => "ReplaceGroup",
                    PortCommandKind::SelectGroup => "SelectGroup",
                    PortCommandKind::RemovePairwise => "RemovePairwise",
                    PortCommandKind::RemoveGroup => "RemoveGroup",
                    PortCommandKind::Configure => "Configure",
                    PortCommandKind::CloseGate => "CloseGate",
                    PortCommandKind::SetEdca => "SetEdca",
                    PortCommandKind::SetBssColor => "SetBssColor",
                });
            }
        }
    }
}

/// A fault of one hardware queue: its name, then the queue.
fn queue_fault(path: &mut DiagPath, name: &'static str, queue: u8) {
    path.name(name);
    path.word(u32::from(queue));
}

impl DiagnosticPath for LowerMacFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Tx(error) => {
                path.name("Tx");
                match error {
                    OrdinaryTxError::BufferSizeOverflow => path.name("BufferSizeOverflow"),
                    OrdinaryTxError::UnsupportedPhyRate => path.name("UnsupportedPhyRate"),
                    OrdinaryTxError::DeadlineOverflow => path.name("DeadlineOverflow"),
                    OrdinaryTxError::Tx(error) => {
                        path.name("Tx");
                        error.push_path(path);
                    }
                    OrdinaryTxError::RadioResetRequired(reason) => {
                        path.name("RadioResetRequired");
                        match reason {
                            TxResetReason::CompletionInterruptWithoutState => {
                                path.name("CompletionInterruptWithoutState")
                            }
                            TxResetReason::TimeoutInterruptWithoutState => {
                                path.name("TimeoutInterruptWithoutState")
                            }
                            TxResetReason::ConflictingInterruptEvents(events) => {
                                path.name("ConflictingInterruptEvents");
                                path.word(*events);
                            }
                            TxResetReason::ExecutorDeadline => path.name("ExecutorDeadline"),
                        }
                    }
                    OrdinaryTxError::BlockAckRequestRate => path.name("BlockAckRequestRate"),
                    OrdinaryTxError::InvalidRetryHeader => path.name("InvalidRetryHeader"),
                }
            }
            Self::RxBlockAckReadback => path.name("RxBlockAckReadback"),
            Self::Key(error) => {
                path.name("Key");
                path.name(match error {
                    CryptoKeyError::InvalidGroupKeyId => "InvalidGroupKeyId",
                    CryptoKeyError::InvalidAccessPointAssociationId => {
                        "InvalidAccessPointAssociationId"
                    }
                    CryptoKeyError::Occupied => "Occupied",
                    CryptoKeyError::HardwareRejected => "HardwareRejected",
                });
            }
            Self::RxBlockAck(error) => {
                path.name("RxBlockAck");
                match error {
                    RxBlockAckError::HardwareIndex(index) => {
                        path.name("HardwareIndex");
                        path.word(u32::from(*index));
                    }
                    RxBlockAckError::MulticastPeer => path.name("MulticastPeer"),
                    RxBlockAckError::Tid(tid) => {
                        path.name("Tid");
                        path.word(u32::from(*tid));
                    }
                    RxBlockAckError::Window(window) => {
                        path.name("Window");
                        path.word(u32::from(*window));
                    }
                    RxBlockAckError::HardwareReadbackMismatch => {
                        path.name("HardwareReadbackMismatch")
                    }
                }
            }
            Self::NoRxBlockAckBank => path.name("NoRxBlockAckBank"),
            Self::Abandoned => path.name("Abandoned"),
            Self::KeyUseMismatch => path.name("KeyUseMismatch"),
            Self::KeyBank => path.name("KeyBank"),
        }
    }
}

impl DiagnosticPath for TxError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Invalid => path.name("Invalid"),
            Self::QueueActive => path.name("QueueActive"),
            Self::Stale => path.name("Stale"),
            Self::DetachFailed => path.name("DetachFailed"),
            Self::TimeoutNotPending => path.name("TimeoutNotPending"),
            Self::ResetRequired => path.name("ResetRequired"),
            Self::Completion(fault) => {
                path.name("Completion");
                fault.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for CompletionFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::SecurityKeyError => path.name("SecurityKeyError"),
            Self::InvalidStatus { status } => {
                path.name("InvalidStatus");
                path.word(u32::from(*status));
            }
        }
    }
}

impl DiagnosticPath for AggregateTxError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::NotFree(state) => {
                path.name("NotFree");
                path.name(match state {
                    TxDmaState::Free => "Free",
                    TxDmaState::Reserved => "Reserved",
                    TxDmaState::HardwareOwned => "HardwareOwned",
                    TxDmaState::Completed => "Completed",
                    TxDmaState::ResetRequired => "ResetRequired",
                });
            }
            Self::SlotCapacity => path.name("SlotCapacity"),
            Self::Stale => path.name("Stale"),
            Self::Completion(fault) => {
                path.name("Completion");
                fault.push_path(path);
            }
            Self::FrameIndexOutOfRange { index, count } => {
                path.name("FrameIndexOutOfRange");
                path.word(u32::from(*index));
                path.word(u32::from(*count));
            }
            Self::BackingUnavailable { index } => {
                path.name("BackingUnavailable");
                path.word(u32::from(*index));
            }
            Self::SlotCountOverflow { count } => {
                path.name("SlotCountOverflow");
                path.word(u32::try_from(*count).unwrap_or(u32::MAX));
            }
            Self::InvalidStorageGeometry { slots } => {
                path.name("InvalidStorageGeometry");
                path.word(u32::try_from(*slots).unwrap_or(u32::MAX));
            }
            Self::TxProgramUnavailable { format } => {
                path.name("TxProgramUnavailable");
                path.name(match format {
                    AggregateFormat::Ht => "Ht",
                    AggregateFormat::He => "He",
                });
            }
            Self::InvalidTriggerReservation { queue, subframes } => {
                path.name("InvalidTriggerReservation");
                path.word(u32::try_from(*queue).unwrap_or(u32::MAX));
                path.word(u32::from(*subframes));
            }
            Self::FrameTooLong => path.name("FrameTooLong"),
            Self::TooFewFrames => path.name("TooFewFrames"),
            Self::AggregateFull => path.name("AggregateFull"),
            Self::Length(error) => {
                path.name("Length");
                match error {
                    AggregateLengthError::InvalidLimits => path.name("InvalidLimits"),
                    AggregateLengthError::Empty => path.name("Empty"),
                    AggregateLengthError::ZeroMpduLength => path.name("ZeroMpduLength"),
                    AggregateLengthError::WindowFull => path.name("WindowFull"),
                    AggregateLengthError::AggregateTooLong(length) => {
                        path.name("AggregateTooLong");
                        path.word(*length);
                    }
                }
            }
            Self::AggregateConfigurationMismatch => path.name("AggregateConfigurationMismatch"),
            Self::QueueActive => path.name("QueueActive"),
            Self::TriggerMsduLengthUnavailable => path.name("TriggerMsduLengthUnavailable"),
            Self::TriggerBased(_) => {
                path.name("TriggerBased");
                path.truncated();
            }
            Self::DetachFailed => path.name("DetachFailed"),
            Self::TimeoutNotPending => path.name("TimeoutNotPending"),
            Self::ResetRequired => path.name("ResetRequired"),
            Self::DmaStorage(error) => {
                path.name("DmaStorage");
                path.name(match error {
                    ChainStorageError::Address => "Address",
                    ChainStorageError::Count => "Count",
                    ChainStorageError::GenerationExhausted => "GenerationExhausted",
                    ChainStorageError::InvalidLength => "InvalidLength",
                    ChainStorageError::StaleBacking => "StaleBacking",
                    ChainStorageError::State => "State",
                    ChainStorageError::Retired => "Retired",
                });
            }
        }
    }
}

impl DiagnosticPath for LifecycleFailure {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::StartRefused(refused) => {
                path.name("StartRefused");
                path.name(match refused {
                    oer_protocols_ieee80211_contracts_port::StartRefused::Running => "Running",
                    oer_protocols_ieee80211_contracts_port::StartRefused::Retune => "Retune",
                    oer_protocols_ieee80211_contracts_port::StartRefused::Deadline => "Deadline",
                    oer_protocols_ieee80211_contracts_port::StartRefused::Device => "Device",
                });
            }
            Self::StopUnconfirmed(observed) => {
                path.name("StopUnconfirmed");
                path.word(u32::from(observed.deadline));
                path.word(u32::from(observed.arena_on_air));
                path.word(u32::from(observed.frames_out));
                if let Some(port) = &observed.port {
                    stop_unconfirmed(port, path);
                }
            }
            Self::RetuneUnconfirmed(unconfirmed) => {
                path.name("RetuneUnconfirmed");
                stop_unconfirmed(unconfirmed, path);
            }
            Self::Port(fault) => {
                path.name("Port");
                fault.push_path(path);
            }
            Self::Actor(violation) => {
                path.name("Actor");
                path.name(actor_violation(*violation));
            }
            Self::Contract(violation) => {
                path.name("Contract");
                path.name(lifecycle_violation(*violation));
                if let oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation::Returned(_) =
                    violation
                {
                    path.truncated();
                }
            }
            Self::Reinstall(cause) => {
                path.name("Reinstall");
                path.name(match cause {
                    ReinstallCause::NotRetired => "NotRetired",
                    ReinstallCause::SameEpoch => "SameEpoch",
                    ReinstallCause::ChannelUnset => "ChannelUnset",
                    ReinstallCause::Receipt => "Receipt",
                    ReinstallCause::Resources => "Resources",
                    ReinstallCause::Epochs => "Epochs",
                });
            }
            Self::Mailbox => path.name("Mailbox"),
        }
    }
}

/// A port's unconfirmed stop: its cause, then what it still held.
fn stop_unconfirmed(
    unconfirmed: &oer_protocols_ieee80211_contracts_port::StopUnconfirmed,
    path: &mut DiagPath,
) {
    path.truncated();
    let observations = unconfirmed.observations;
    path.word(u32::from(observations.arena_on_air));
    path.word(u32::from(observations.published));
    path.word(u32::from(observations.queued));
    path.word(u32::from(observations.configuring));
}

fn actor_violation(
    violation: oer_protocols_ieee80211_runtime::datapath::ActorViolation,
) -> &'static str {
    use oer_protocols_ieee80211_runtime::datapath::ActorViolation as V;
    match violation {
        V::OutboxFull => "OutboxFull",
        V::ArenaClaim => "ArenaClaim",
        V::ArenaCompletion => "ArenaCompletion",
        V::LinkMailbox => "LinkMailbox",
        V::ParcelMissing => "ParcelMissing",
        V::RetireRunning => "RetireRunning",
        V::LinkReturn => "LinkReturn",
        V::PairwiseCustody => "PairwiseCustody",
        V::CutOverflow => "CutOverflow",
        V::ReorderOverflow => "ReorderOverflow",
    }
}

fn lifecycle_violation(
    violation: oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation,
) -> &'static str {
    use oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation as V;
    match violation {
        V::Returned(_) => "Returned",
        V::Missing => "Missing",
        V::QueueFull => "QueueFull",
        V::RetuneWithoutRequest => "RetuneWithoutRequest",
    }
}
