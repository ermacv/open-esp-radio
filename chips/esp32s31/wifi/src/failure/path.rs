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
        }
    }
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
