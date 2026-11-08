//! The variant paths a failed station's record keeps: the lifecycle's
//! cause, then the port's named fault or the observations it kept. A
//! subtree the path does not spell out marks the path cut
//! (`DiagPath::truncated`).
use super::*;
use oer_portable_diagnostics::{DiagPath, DiagnosticPath};

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
            Self::Unpublished(cause) => {
                path.name("Unpublished");
                path.name(match cause {
                    UnpublishedCause::CoreView => "CoreView",
                    UnpublishedCause::Admission => "Admission",
                    UnpublishedCause::Key => "Key",
                    UnpublishedCause::Step(step) => publish_step(step),
                });
            }
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
            Self::HardwareUnavailable => path.name("HardwareUnavailable"),
            Self::CoreViewUnavailable(op) => {
                path.name("CoreViewUnavailable");
                path.name(match op {
                    CoreOperation::Command => "Command",
                    CoreOperation::Tbtt => "Tbtt",
                    CoreOperation::Arena => "Arena",
                    CoreOperation::Enable => "Enable",
                    CoreOperation::Configure => "Configure",
                    CoreOperation::Open => "Open",
                });
            }
            Self::Publish { queue, step } => {
                queue_fault(path, "Publish", *queue);
                path.name(publish_step(step));
            }
            Self::DuplicateCompletion { queue } => queue_fault(path, "DuplicateCompletion", *queue),
            Self::RemovalOverflow => path.name("RemovalOverflow"),
            Self::RemovalMissing => path.name("RemovalMissing"),
            Self::RemovalMismatch => path.name("RemovalMismatch"),
            Self::GroupKeyIdRange => path.name("GroupKeyIdRange"),
            Self::ConfigurationRefused => path.name("ConfigurationRefused"),
            Self::CompletionWithoutPending { queue } => {
                queue_fault(path, "CompletionWithoutPending", *queue);
            }
            Self::DescriptorMissing { queue } => queue_fault(path, "DescriptorMissing", *queue),
            Self::ReturnRefused { queue } => queue_fault(path, "ReturnRefused", *queue),
            Self::QueueFull { queue } => queue_fault(path, "QueueFull", *queue),
            Self::ReceiveOverflow => path.name("ReceiveOverflow"),
            Self::ReceiveRange => path.name("ReceiveRange"),
            Self::ReceiveLost => path.name("ReceiveLost"),
            Self::ArenaCoreContract => path.name("ArenaCoreContract"),
            Self::InstallationResources => path.name("InstallationResources"),
            Self::EdcaUnsupported => path.name("EdcaUnsupported"),
            Self::ChannelNotPermitted => path.name("ChannelNotPermitted"),
            Self::ChannelUnsupported => path.name("ChannelUnsupported"),
            Self::KeySlotMissing(op) => {
                path.name("KeySlotMissing");
                path.name(match op {
                    KeyOperation::Install => "Install",
                    KeyOperation::Remove => "Remove",
                    KeyOperation::Replace => "Replace",
                    KeyOperation::Select => "Select",
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
                    PortCommandKind::SetHe => "SetHe",
                });
            }
            Self::HeAssociationRange => path.name("HeAssociationRange"),
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
                    RxBlockAckError::Domain => path.name("Domain"),
                }
            }
            Self::NoRxBlockAckBank => path.name("NoRxBlockAckBank"),
            Self::Abandoned => path.name("Abandoned"),
            Self::KeyUseMismatch => path.name("KeyUseMismatch"),
            Self::KeyBank => path.name("KeyBank"),
        }
    }
}

fn publish_step(step: &PublishStep) -> &'static str {
    match step {
        PublishStep::Observations => "Observations",
        PublishStep::Ticket => "Ticket",
        PublishStep::KeyAdmission => "KeyAdmission",
        PublishStep::ReceiveKey => "ReceiveKey",
        PublishStep::Buffer => "Buffer",
        PublishStep::Descriptor => "Descriptor",
        PublishStep::Attach => "Attach",
        PublishStep::DescriptorBusy => "DescriptorBusy",
        PublishStep::HeaderBuffer => "HeaderBuffer",
        PublishStep::CcmpHeader => "CcmpHeader",
        PublishStep::Values => "Values",
        PublishStep::Queue => "Queue",
    }
}

impl DiagnosticPath for TxPrepareFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::ForeignProgram => path.name("ForeignProgram"),
            Self::ChainNotNamed { .. } => path.name("ChainNotNamed"),
            Self::Unsupported => path.name("Unsupported"),
        }
    }
}

impl DiagnosticPath for TxError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Invalid => path.name("Invalid"),
            Self::QueueActive => path.name("QueueActive"),
            Self::Prepare(refused) => {
                path.name("Prepare");
                refused.push_path(path);
            }
            Self::Stale => path.name("Stale"),
            Self::DetachFailed => path.name("DetachFailed"),
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
            Self::Prepare(refused) => {
                path.name("Prepare");
                refused.push_path(path);
            }
            Self::TriggerMsduLengthUnavailable => path.name("TriggerMsduLengthUnavailable"),
            Self::TriggerBased(_) => {
                path.name("TriggerBased");
                path.truncated();
            }
            Self::DetachFailed => path.name("DetachFailed"),
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
                use oer_protocols_ieee80211_contracts_port::StartRefused as Refused;
                path.name("StartRefused");
                match refused {
                    Refused::Running => path.name("Running"),
                    Refused::Retune(cause) => {
                        path.name("Retune");
                        cause.push_path(path);
                    }
                    Refused::Deadline => path.name("Deadline"),
                    Refused::Device => path.name("Device"),
                }
            }
            Self::StopUnconfirmed(observed) => {
                path.name("StopUnconfirmed");
                // Four flags in one word: a path keeps a few words only.
                path.word(
                    u32::from(observed.deadline)
                        | u32::from(observed.arena_on_air) << 1
                        | u32::from(observed.network_running) << 2
                        | u32::from(observed.network_lost) << 3,
                );
                path.word(u32::from(observed.frames_out));
                path.word(u32::from(observed.lent));
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
