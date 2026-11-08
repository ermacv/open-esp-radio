//! Exhaustive conversion at the facade's target boundary.
use super::*;
pub(crate) fn backend_fault(
    value: oer_chips_esp32s31_radio_ieee80211_backend::port::Fault,
) -> BackendFault {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::Fault as Source;
    match value {
        Source::Core(v0) => BackendFault::Core(lower_mac_fault(v0)),
        Source::Receive(v0) => BackendFault::Receive(receive_fault(v0)),
        Source::Unpublished(cause) => BackendFault::Unpublished(match cause {
            oer_chips_esp32s31_radio_ieee80211_backend::port::Unpublished::CoreView => {
                UnpublishedCause::CoreView
            }
            oer_chips_esp32s31_radio_ieee80211_backend::port::Unpublished::Admission => {
                UnpublishedCause::Admission
            }
            oer_chips_esp32s31_radio_ieee80211_backend::port::Unpublished::Key => {
                UnpublishedCause::Key
            }
            oer_chips_esp32s31_radio_ieee80211_backend::port::Unpublished::Step(step) => {
                UnpublishedCause::Step(publish_step(step))
            }
        }),
        Source::Retune(v0) => BackendFault::Retune(retune_error(v0)),
        Source::Abandoned => BackendFault::Abandoned,
        Source::Arena(v0) => BackendFault::Arena(arena_fault(v0)),
        Source::TuneNotRecorded => BackendFault::TuneNotRecorded,
        Source::HardwareUnavailable => BackendFault::HardwareUnavailable,
        Source::CoreViewUnavailable(op) => BackendFault::CoreViewUnavailable(core_operation(op)),
        Source::Publish { queue, step } => BackendFault::Publish {
            queue,
            step: publish_step(step),
        },
        Source::DuplicateCompletion { queue } => BackendFault::DuplicateCompletion { queue },
        Source::RemovalOverflow => BackendFault::RemovalOverflow,
        Source::RemovalMissing => BackendFault::RemovalMissing,
        Source::RemovalMismatch => BackendFault::RemovalMismatch,
        Source::GroupKeyIdRange => BackendFault::GroupKeyIdRange,
        Source::ConfigurationRefused => BackendFault::ConfigurationRefused,
        Source::CompletionWithoutPending { queue } => {
            BackendFault::CompletionWithoutPending { queue }
        }
        Source::DescriptorMissing { queue } => BackendFault::DescriptorMissing { queue },
        Source::ReturnRefused { queue } => BackendFault::ReturnRefused { queue },
        Source::QueueFull { queue } => BackendFault::QueueFull { queue },
        Source::ReceiveOverflow => BackendFault::ReceiveOverflow,
        Source::ReceiveRange => BackendFault::ReceiveRange,
        Source::ReceiveLost => BackendFault::ReceiveLost,
        Source::ArenaCoreContract => BackendFault::ArenaCoreContract,
        Source::InstallationResources => BackendFault::InstallationResources,
        Source::EdcaUnsupported => BackendFault::EdcaUnsupported,
        Source::ChannelNotPermitted => BackendFault::ChannelNotPermitted,
        Source::ChannelUnsupported => BackendFault::ChannelUnsupported,
        Source::KeySlotMissing(op) => BackendFault::KeySlotMissing(key_operation(op)),
        Source::ReceiveAccounting => BackendFault::ReceiveAccounting,
        Source::CommandExpired(kind) => BackendFault::CommandExpired(command_kind(kind)),
    }
}

/// A refused start, with a refused retune's cause.
pub(crate) fn start_refused(
    value: oer_protocols_ieee80211_contracts_port::StartRefused<
        oer_chips_esp32s31_radio_ieee80211_backend::port::Fault,
    >,
) -> oer_protocols_ieee80211_contracts_port::StartRefused<BackendFault> {
    use oer_protocols_ieee80211_contracts_port::StartRefused as Source;
    match value {
        Source::Running => Source::Running,
        Source::Retune(cause) => Source::Retune(backend_fault(cause)),
        Source::Deadline => Source::Deadline,
        Source::Device => Source::Device,
    }
}

pub(super) fn core_operation(
    value: oer_chips_esp32s31_radio_ieee80211_backend::port::CoreOp,
) -> CoreOperation {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::CoreOp as Source;
    match value {
        Source::Command => CoreOperation::Command,
        Source::Tbtt => CoreOperation::Tbtt,
        Source::Arena => CoreOperation::Arena,
        Source::Enable => CoreOperation::Enable,
        Source::Configure => CoreOperation::Configure,
        Source::Open => CoreOperation::Open,
    }
}

pub(super) fn publish_step(
    value: oer_chips_esp32s31_radio_ieee80211_backend::port::PublishStep,
) -> PublishStep {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::PublishStep as Source;
    match value {
        Source::Observations => PublishStep::Observations,
        Source::Ticket => PublishStep::Ticket,
        Source::KeyAdmission => PublishStep::KeyAdmission,
        Source::ReceiveKey => PublishStep::ReceiveKey,
        Source::Buffer => PublishStep::Buffer,
        Source::Descriptor => PublishStep::Descriptor,
        Source::Attach => PublishStep::Attach,
        Source::DescriptorBusy => PublishStep::DescriptorBusy,
        Source::HeaderBuffer => PublishStep::HeaderBuffer,
        Source::CcmpHeader => PublishStep::CcmpHeader,
        Source::Values => PublishStep::Values,
        Source::Queue => PublishStep::Queue,
    }
}

pub(super) fn key_operation(
    value: oer_chips_esp32s31_radio_ieee80211_backend::port::KeyOp,
) -> KeyOperation {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::KeyOp as Source;
    match value {
        Source::Install => KeyOperation::Install,
        Source::Remove => KeyOperation::Remove,
        Source::Replace => KeyOperation::Replace,
        Source::Select => KeyOperation::Select,
    }
}

pub(super) fn command_kind(
    value: oer_protocols_ieee80211_contracts_port::CommandKind,
) -> PortCommandKind {
    use oer_protocols_ieee80211_contracts_port::CommandKind as Source;
    match value {
        Source::InstallPairwise => PortCommandKind::InstallPairwise,
        Source::InstallGroup => PortCommandKind::InstallGroup,
        Source::ReplaceGroup => PortCommandKind::ReplaceGroup,
        Source::SelectGroup => PortCommandKind::SelectGroup,
        Source::RemovePairwise => PortCommandKind::RemovePairwise,
        Source::RemoveGroup => PortCommandKind::RemoveGroup,
        Source::Configure => PortCommandKind::Configure,
        Source::CloseGate => PortCommandKind::CloseGate,
        Source::SetEdca => PortCommandKind::SetEdca,
        Source::SetBssColor => PortCommandKind::SetBssColor,
    }
}

pub(super) fn aggregate_tx_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduTxError,
) -> AggregateTxError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduTxError as Source;
    match value {
        Source::NotFree(v0) => AggregateTxError::NotFree(tx_dma_state(v0)),
        Source::SlotCapacity => AggregateTxError::SlotCapacity,
        Source::Stale => AggregateTxError::Stale,
        Source::Completion(v0) => AggregateTxError::Completion(completion_fault(v0)),
        Source::FrameIndexOutOfRange { index, count } => {
            AggregateTxError::FrameIndexOutOfRange { index, count }
        }
        Source::BackingUnavailable { index } => AggregateTxError::BackingUnavailable { index },
        Source::SlotCountOverflow { count } => AggregateTxError::SlotCountOverflow { count },
        Source::InvalidStorageGeometry { slots } => {
            AggregateTxError::InvalidStorageGeometry { slots }
        }
        Source::TxProgramUnavailable { format } => AggregateTxError::TxProgramUnavailable {
            format: aggregate_format(format),
        },
        Source::InvalidTriggerReservation { queue, subframes } => {
            AggregateTxError::InvalidTriggerReservation {
                queue: usize::from(queue),
                subframes,
            }
        }
        Source::FrameTooLong => AggregateTxError::FrameTooLong,
        Source::TooFewFrames => AggregateTxError::TooFewFrames,
        Source::AggregateFull => AggregateTxError::AggregateFull,
        Source::Length(v0) => AggregateTxError::Length(aggregate_length_error(v0)),
        Source::AggregateConfigurationMismatch => AggregateTxError::AggregateConfigurationMismatch,
        Source::QueueActive => AggregateTxError::QueueActive,
        Source::Prepare(v0) => AggregateTxError::Prepare(tx_prepare_fault(v0)),
        Source::TriggerMsduLengthUnavailable => AggregateTxError::TriggerMsduLengthUnavailable,
        Source::TriggerBased(v0) => AggregateTxError::TriggerBased(trigger_program_error(v0)),
        Source::DetachFailed => AggregateTxError::DetachFailed,
        Source::ResetRequired => AggregateTxError::ResetRequired,
        Source::DmaStorage(v0) => AggregateTxError::DmaStorage(chain_storage_error(v0)),
    }
}

pub(super) fn tx_dma_state(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxDmaState,
) -> TxDmaState {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxDmaState as Source;
    match value {
        Source::Free => TxDmaState::Free,
        Source::Reserved => TxDmaState::Reserved,
        Source::HardwareOwned => TxDmaState::HardwareOwned,
        Source::Completed => TxDmaState::Completed,
        Source::ResetRequired => TxDmaState::ResetRequired,
    }
}

pub(super) fn aggregate_format(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduTxFormat,
) -> AggregateFormat {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduTxFormat as Source;
    match value {
        Source::HtAmpdu => AggregateFormat::Ht,
        Source::HeAmpdu => AggregateFormat::He,
    }
}

pub(super) fn aggregate_length_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduLengthError,
) -> AggregateLengthError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::ampdu::HtAmpduLengthError as Source;
    match value {
        Source::InvalidLimits => AggregateLengthError::InvalidLimits,
        Source::Empty => AggregateLengthError::Empty,
        Source::ZeroMpduLength => AggregateLengthError::ZeroMpduLength,
        Source::WindowFull => AggregateLengthError::WindowFull,
        Source::AggregateTooLong(v0) => AggregateLengthError::AggregateTooLong(v0),
    }
}

pub(super) fn trigger_program_error(
    value: oer_chips_esp32s31_hal::types::MacHeTbProgramError,
) -> TriggerProgramError {
    use oer_chips_esp32s31_hal::types::MacHeTbProgramError as Source;
    match value {
        Source::TidNotEligible => TriggerProgramError::TidNotEligible,
        Source::LengthCountMismatch => TriggerProgramError::LengthCountMismatch,
        Source::InvalidMpduLength => TriggerProgramError::InvalidMpduLength,
        Source::QueuedBytesTooLarge => TriggerProgramError::QueuedBytesTooLarge,
    }
}

pub(super) fn chain_storage_error(
    value: oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_ampdu_storage::AmpduDmaStorageError,
) -> ChainStorageError {
    use oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_ampdu_storage::AmpduDmaStorageError as Source;
    match value {
        Source::Address => ChainStorageError::Address,
        Source::Count => ChainStorageError::Count,
        Source::GenerationExhausted => ChainStorageError::GenerationExhausted,
        Source::InvalidLength => ChainStorageError::InvalidLength,
        Source::StaleBacking => ChainStorageError::StaleBacking,
        Source::State => ChainStorageError::State,
        Source::Retired => ChainStorageError::Retired,
    }
}

pub(super) fn arena_fault(
    value: oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::arena::ArenaFault,
) -> ArenaFault {
    use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::arena::ArenaFault as Source;
    match value {
        Source::Chain(v0) => ArenaFault::Chain(aggregate_tx_error(v0)),
        Source::Core(v0) => ArenaFault::Core(lower_mac_fault(v0)),
        Source::State => ArenaFault::State,
        Source::Watchdog => ArenaFault::Watchdog,
    }
}

pub(super) fn lower_mac_fault(
    value: oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::LowerMacFault,
) -> LowerMacFault {
    use oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::LowerMacFault as Source;
    match value {
        Source::Tx(v0) => LowerMacFault::Tx(ordinary_tx_error(v0)),
        Source::RxBlockAckReadback => LowerMacFault::RxBlockAckReadback,
        Source::Key(v0) => LowerMacFault::Key(crypto_key_error(v0)),
        Source::RxBlockAck(v0) => LowerMacFault::RxBlockAck(rx_block_ack_error(v0)),
        Source::NoRxBlockAckBank => LowerMacFault::NoRxBlockAckBank,
        Source::Abandoned => LowerMacFault::Abandoned,
        Source::KeyUseMismatch => LowerMacFault::KeyUseMismatch,
        Source::KeyBank => LowerMacFault::KeyBank,
    }
}

pub(super) fn ordinary_tx_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::ordinary_tx::OrdinaryTxError,
) -> OrdinaryTxError {
    use oer_chips_esp32s31_radio_ieee80211_backend::ordinary_tx::OrdinaryTxError as Source;
    match value {
        Source::BufferSizeOverflow => OrdinaryTxError::BufferSizeOverflow,
        Source::UnsupportedPhyRate => OrdinaryTxError::UnsupportedPhyRate,
        Source::DeadlineOverflow => OrdinaryTxError::DeadlineOverflow,
        Source::Tx(v0) => OrdinaryTxError::Tx(tx_error(v0)),
        Source::RadioResetRequired(v0) => OrdinaryTxError::RadioResetRequired(tx_reset_reason(v0)),
        Source::BlockAckRequestRate => OrdinaryTxError::BlockAckRequestRate,
        Source::InvalidRetryHeader => OrdinaryTxError::InvalidRetryHeader,
    }
}

fn tx_prepare_fault(value: oer_chips_esp32s31_hal::types::MacTxPrepareFault) -> TxPrepareFault {
    use oer_chips_esp32s31_hal::types::MacTxPrepareFault as Source;
    match value {
        Source::ForeignProgram => TxPrepareFault::ForeignProgram,
        Source::ChainNotNamed { named } => TxPrepareFault::ChainNotNamed { named },
        Source::Unsupported => TxPrepareFault::Unsupported,
    }
}

pub(super) fn tx_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxError,
) -> TxError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxError as Source;
    match value {
        Source::Invalid => TxError::Invalid,
        Source::QueueActive => TxError::QueueActive,
        Source::Prepare(v0) => TxError::Prepare(tx_prepare_fault(v0)),
        Source::Stale => TxError::Stale,
        Source::DetachFailed => TxError::DetachFailed,
        Source::Completion(v0) => TxError::Completion(completion_fault(v0)),
    }
}

pub(super) fn completion_fault(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::CompletionFault,
) -> CompletionFault {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::CompletionFault as Source;
    match value {
        Source::InvalidStatus { status } => CompletionFault::InvalidStatus { status },
    }
}

pub(super) fn tx_reset_reason(
    value: oer_chips_esp32s31_radio_ieee80211_backend::ordinary_tx::TxResetReason,
) -> TxResetReason {
    use oer_chips_esp32s31_radio_ieee80211_backend::ordinary_tx::TxResetReason as Source;
    match value {
        Source::CompletionInterruptWithoutState => TxResetReason::CompletionInterruptWithoutState,
        Source::TimeoutInterruptWithoutState => TxResetReason::TimeoutInterruptWithoutState,
        Source::ConflictingInterruptEvents(v0) => TxResetReason::ConflictingInterruptEvents(v0),
        Source::ExecutorDeadline => TxResetReason::ExecutorDeadline,
    }
}

pub(super) fn crypto_key_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::crypto::CryptoKeyError,
) -> CryptoKeyError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::crypto::CryptoKeyError as Source;
    match value {
        Source::InvalidGroupKeyId => CryptoKeyError::InvalidGroupKeyId,
        Source::InvalidAccessPointAssociationId => CryptoKeyError::InvalidAccessPointAssociationId,
        Source::Occupied => CryptoKeyError::Occupied,
        Source::HardwareRejected => CryptoKeyError::HardwareRejected,
    }
}

pub(super) fn receive_fault(
    value: oer_chips_esp32s31_radio_ieee80211_backend::rx::control::WifiRxCause,
) -> ReceiveFault {
    use oer_chips_esp32s31_radio_ieee80211_backend::rx::control::WifiRxCause as Source;
    match value {
        Source::StopRequested => ReceiveFault::StopRequested,
        Source::Prepare(v0) => ReceiveFault::Prepare(rx_ring_error(v0)),
        Source::LineRefused => ReceiveFault::LineRefused,
        Source::Start(v0) => ReceiveFault::Start(rx_frontier_error(v0)),
        Source::Service(v0) => ReceiveFault::Service(rx_stage_transaction_error(v0)),
        Source::ForeignExecutor => ReceiveFault::ForeignExecutor,
    }
}

pub(super) fn rx_ring_error(
    value: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingError,
) -> RxRingError {
    use oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingError as Source;
    match value {
        Source::Empty => RxRingError::Empty,
        Source::Count => RxRingError::Count,
        Source::Address => RxRingError::Address,
        Source::DescriptorOwnerAddress => RxRingError::DescriptorOwnerAddress,
        Source::Size => RxRingError::Size,
        Source::Overflow => RxRingError::Overflow,
        Source::OutstandingLeases => RxRingError::OutstandingLeases,
        Source::BufferOwnershipConflict => RxRingError::BufferOwnershipConflict,
        Source::ReloadPending => RxRingError::ReloadPending,
        Source::WalkerStartUnconfirmed => RxRingError::WalkerStartUnconfirmed,
        Source::WalkerStopUnconfirmed => RxRingError::WalkerStopUnconfirmed,
        Source::Corrupt => RxRingError::Corrupt,
        Source::ResetRequired => RxRingError::ResetRequired,
    }
}

pub(super) fn rx_frontier_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::rx::frontier::RxFrontierError,
) -> RxFrontierError {
    use oer_chips_esp32s31_radio_ieee80211_backend::rx::frontier::RxFrontierError as Source;
    match value {
        Source::AlreadyStarted => RxFrontierError::AlreadyStarted,
        Source::OwnerUnavailable => RxFrontierError::OwnerUnavailable,
        Source::Ring(v0) => RxFrontierError::Ring(rx_ring_error(v0)),
    }
}

pub(super) fn rx_stage_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::pool::RxStageError,
) -> RxStageError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::pool::RxStageError as Source;
    match value {
        Source::Exhausted => RxStageError::Exhausted,
        Source::Empty => RxStageError::Empty,
        Source::TooLong => RxStageError::TooLong,
        Source::Chained => RxStageError::Chained,
    }
}

pub(super) fn retune_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::port::WifiRetuneError,
) -> RetuneError {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::WifiRetuneError as Source;
    match value {
        Source::NotStopped(v0) => RetuneError::NotStopped(maintenance_error(v0)),
        Source::Radio(v0) => RetuneError::Radio(radio_failure(v0)),
    }
}

pub(super) fn maintenance_error(
    value: oer_chips_esp32s31_hal::system::owner::maintenance::Error,
) -> MaintenanceError {
    use oer_chips_esp32s31_hal::system::owner::maintenance::Error as Source;
    match value {
        Source::MacActive { state } => MaintenanceError::MacActive { state },
        Source::RxWalkerEnabled => MaintenanceError::RxWalkerEnabled,
    }
}

pub(super) fn radio_failure(value: oer_chips_esp32s31_radio_client::Failure) -> RadioFailure {
    use oer_chips_esp32s31_radio_client::Failure as Source;
    match value {
        Source::Refused => RadioFailure::Refused,
        Source::ClientFailed => RadioFailure::ClientFailed,
        Source::PhyRefused(v0) => RadioFailure::PhyRefused(phy_domain_error(v0)),
        Source::PoisonedAfterRefusal { refusal, poison } => RadioFailure::PoisonedAfterRefusal {
            refusal: phy_domain_error(refusal),
            poison: radio_poison(poison),
        },
        Source::Poisoned(v0) => RadioFailure::Poisoned(radio_poison(v0)),
    }
}

pub(super) fn radio_poison(value: oer_chips_esp32s31_radio_client::Poison) -> RadioPoison {
    let oer_chips_esp32s31_radio_client::Poison { origin, cause } = value;
    RadioPoison {
        origin: radio_operation(origin),
        cause: radio_poison_cause(cause),
    }
}

pub(super) fn radio_operation(
    value: oer_chips_esp32s31_radio_client::PoisonOrigin,
) -> RadioOperation {
    use oer_chips_esp32s31_radio_client::PoisonOrigin as Source;
    match value {
        Source::TrackingTick => RadioOperation::TrackingTick,
        Source::Prepare => RadioOperation::Prepare,
        Source::CloseRf => RadioOperation::CloseRf,
        Source::Request { client, request } => RadioOperation::Request {
            client: radio_client(client),
            request: radio_request(request),
        },
    }
}

pub(super) fn radio_request(value: oer_chips_esp32s31_radio_client::RequestKind) -> RadioRequest {
    use oer_chips_esp32s31_radio_client::RequestKind as Source;
    match value {
        Source::Join => RadioRequest::Join,
        Source::Track => RadioRequest::Track,
        Source::Retune => RadioRequest::Retune,
        Source::Leave => RadioRequest::Leave,
    }
}

pub(super) fn radio_client(
    value: oer_chips_esp32s31_hal::radio::domain::RadioClient,
) -> RadioClient {
    use oer_chips_esp32s31_hal::radio::domain::RadioClient as Source;
    match value {
        Source::Wifi => RadioClient::Wifi,
        Source::Bluetooth => RadioClient::Bluetooth,
        Source::Ieee802154 => RadioClient::Ieee802154,
    }
}

pub(super) fn radio_poison_cause(
    value: oer_chips_esp32s31_radio_client::PoisonCause,
) -> RadioPoisonCause {
    use oer_chips_esp32s31_radio_client::PoisonCause as Source;
    match value {
        Source::TrackingRefused(v0) => RadioPoisonCause::TrackingRefused(phy_domain_error(v0)),
        Source::Tracking(v0) => RadioPoisonCause::Tracking(phy_fault(v0)),
        Source::CalibrationTracking(v0) => {
            RadioPoisonCause::CalibrationTracking(calibration_tracking_fault(v0))
        }
        Source::Channel(v0) => RadioPoisonCause::Channel(phy_fault(v0)),
        Source::Rf(v0) => RadioPoisonCause::Rf(phy_fault(v0)),
        Source::Registration(v0) => RadioPoisonCause::Registration(registration_fault(v0)),
        Source::Domain(v0) => RadioPoisonCause::Domain(phy_domain_error(v0)),
        Source::CommonPower(v0) => RadioPoisonCause::CommonPower(common_radio_power_error(v0)),
        Source::ModemClock(v0) => RadioPoisonCause::ModemClock(modem_clock_error(v0)),
        Source::Rollback => RadioPoisonCause::Rollback,
        Source::RollbackAfterRefusal(v0) => {
            RadioPoisonCause::RollbackAfterRefusal(phy_domain_error(v0))
        }
        Source::Controller => RadioPoisonCause::Controller,
        Source::Custody => RadioPoisonCause::Custody,
    }
}

pub(super) fn phy_fault(value: oer_chips_esp32s31_radio_client::PhyFault) -> PhyFault {
    use oer_chips_esp32s31_radio_client::PhyFault as Source;
    match value {
        Source::EdgeTimedOut => PhyFault::EdgeTimedOut,
        Source::CapabilityUnavailable => PhyFault::CapabilityUnavailable,
        Source::HardwareInvariant => PhyFault::HardwareInvariant,
        Source::RfOperationLimit => PhyFault::RfOperationLimit,
        Source::UnexpectedBinding => PhyFault::UnexpectedBinding,
        Source::RegistrationEpochMismatch => PhyFault::RegistrationEpochMismatch,
        Source::DeadlineOverflow => PhyFault::DeadlineOverflow,
        Source::Sequence => PhyFault::Sequence,
    }
}

pub(super) fn calibration_tracking_fault(
    value: oer_chips_esp32s31_radio_client::CalibrationTrackingFault,
) -> CalibrationTrackingFault {
    use oer_chips_esp32s31_radio_client::CalibrationTrackingFault as Source;
    match value {
        Source::Step(v0) => CalibrationTrackingFault::Step(phy_fault(v0)),
        Source::Radio(v0) => CalibrationTrackingFault::Radio(calibration_tracking_failure(v0)),
    }
}

pub(super) fn calibration_tracking_failure(
    value: oer_chips_esp32s31_radio_client::CalibrationTrackingFailure,
) -> CalibrationTrackingFailure {
    use oer_chips_esp32s31_radio_client::CalibrationTrackingFailure as Source;
    match value {
        Source::PbusClearTimedOut => CalibrationTrackingFailure::PbusClearTimedOut,
        Source::Dcode => CalibrationTrackingFailure::Dcode,
        Source::RxGain => CalibrationTrackingFailure::RxGain,
        Source::Channel => CalibrationTrackingFailure::Channel,
        Source::TxDcPwdet => CalibrationTrackingFailure::TxDcPwdet,
    }
}

pub(super) fn registration_fault(
    value: oer_chips_esp32s31_radio_client::RegistrationFault,
) -> RegistrationFault {
    use oer_chips_esp32s31_radio_client::RegistrationFault as Source;
    match value {
        Source::Step(v0) => RegistrationFault::Step(phy_fault(v0)),
        Source::Lowering { stage, error } => RegistrationFault::Lowering {
            stage: phy_register_stage(stage),
            error: phy_register_binding_error(error),
        },
        Source::Radio(v0) => RegistrationFault::Radio(registration_failure(v0)),
    }
}

pub(super) fn registration_failure(
    value: oer_chips_esp32s31_radio_client::RegistrationFailure,
) -> RegistrationFailure {
    use oer_chips_esp32s31_radio_client::RegistrationFailure as Source;
    match value {
        Source::I2cMasterResetTimedOut { index, samples } => {
            RegistrationFailure::I2cMasterResetTimedOut { index, samples }
        }
        Source::Rf => RegistrationFailure::Rf,
        Source::Baseband => RegistrationFailure::Baseband,
        Source::Temperature => RegistrationFailure::Temperature,
        Source::FinalI2cDeadlineExceeded => RegistrationFailure::FinalI2cDeadlineExceeded,
    }
}

pub(super) fn phy_register_stage(
    value: oer_chips_esp32s31_radio_phy::PhyRegisterStage,
) -> PhyRegisterStage {
    use oer_chips_esp32s31_radio_phy::PhyRegisterStage as Source;
    match value {
        Source::Mmio => PhyRegisterStage::Mmio,
        Source::Delay => PhyRegisterStage::Delay,
        Source::I2cMasterResetSample => PhyRegisterStage::I2cMasterResetSample,
        Source::Rf => PhyRegisterStage::Rf,
        Source::Baseband => PhyRegisterStage::Baseband,
        Source::Temperature => PhyRegisterStage::Temperature,
        Source::FinalI2cRead => PhyRegisterStage::FinalI2cRead,
    }
}

pub(super) fn phy_register_binding_error(
    value: oer_chips_esp32s31_radio_phy::PhyRegisterBindingError,
) -> PhyRegisterBindingError {
    use oer_chips_esp32s31_radio_phy::PhyRegisterBindingError as Source;
    match value {
        Source::UnsupportedAction => PhyRegisterBindingError::UnsupportedAction,
        Source::IncompleteTransaction => PhyRegisterBindingError::IncompleteTransaction,
        Source::UnexpectedOutcome => PhyRegisterBindingError::UnexpectedOutcome,
    }
}

pub(super) fn phy_domain_error(
    value: oer_chips_esp32s31_radio_phy::concurrent::ConcurrentPhyError,
) -> PhyDomainError {
    use oer_chips_esp32s31_radio_phy::concurrent::ConcurrentPhyError as Source;
    match value {
        Source::NotRegistered => PhyDomainError::NotRegistered,
        Source::AlreadyRegistered => PhyDomainError::AlreadyRegistered,
        Source::TrackingPending => PhyDomainError::TrackingPending,
        Source::NoTrackingPending => PhyDomainError::NoTrackingPending,
        Source::Poisoned => PhyDomainError::Poisoned,
        Source::RfClosed => PhyDomainError::RfClosed,
        Source::RfOpen => PhyDomainError::RfOpen,
        Source::EpochMismatch => PhyDomainError::EpochMismatch,
        Source::ClientsActive => PhyDomainError::ClientsActive,
        Source::Clock(v0) => PhyDomainError::Clock(modem_clock_error(v0)),
        Source::Acquire(v0) => PhyDomainError::Acquire(phy_client_acquire_error(v0)),
        Source::Time(v0) => PhyDomainError::Time(phy_track_time_error(v0)),
        Source::ClientAbsent(v0) => PhyDomainError::ClientAbsent(radio_client(v0)),
        Source::ClientPhase(v0) => PhyDomainError::ClientPhase(radio_client(v0)),
    }
}

pub(super) fn phy_track_time_error(
    value: oer_chips_esp32s31_radio_phy::state::client::PhyTrackTimeError,
) -> PhyTrackTimeError {
    use oer_chips_esp32s31_radio_phy::state::client::PhyTrackTimeError as Source;
    match value {
        Source::DeadlineOverflow { class } => PhyTrackTimeError::DeadlineOverflow {
            class: phy_pll_track_class(class),
        },
        Source::TimeReversed {
            class,
            previous_micros,
            now_micros,
        } => PhyTrackTimeError::TimeReversed {
            class: phy_pll_track_class(class),
            previous_micros,
            now_micros,
        },
    }
}

pub(super) fn phy_client_acquire_error(
    value: oer_chips_esp32s31_radio_phy::state::client::PhyClientAcquireError,
) -> PhyClientAcquireError {
    use oer_chips_esp32s31_radio_phy::state::client::PhyClientAcquireError as Source;
    match value {
        Source::TrackingTime(v0) => PhyClientAcquireError::TrackingTime(phy_track_time_error(v0)),
    }
}

pub(super) fn phy_pll_track_class(
    value: oer_chips_esp32s31_radio_phy::state::client::PhyPllTrackClass,
) -> PhyPllTrackClass {
    use oer_chips_esp32s31_radio_phy::state::client::PhyPllTrackClass as Source;
    match value {
        Source::Wifi => PhyPllTrackClass::Wifi,
        Source::BluetoothIeee802154 => PhyPllTrackClass::BluetoothIeee802154,
    }
}

pub(super) fn common_radio_power_error(
    value: oer_chips_esp32s31_hal::radio::domain::CommonRadioPowerError,
) -> CommonRadioPowerError {
    use oer_chips_esp32s31_hal::radio::domain::CommonRadioPowerError as Source;
    match value {
        Source::AlreadyEntered => CommonRadioPowerError::AlreadyEntered,
        Source::Power(v0) => CommonRadioPowerError::Power(power_error(v0)),
        Source::Restore(v0) => CommonRadioPowerError::Restore(power_restore_checkpoint(v0)),
        Source::Release(v0) => CommonRadioPowerError::Release(platform_clock(v0)),
    }
}

pub(super) fn modem_clock_error(
    value: oer_chips_esp32s31_hal::radio::domain::ModemClockError,
) -> ModemClockError {
    use oer_chips_esp32s31_hal::radio::domain::ModemClockError as Source;
    match value {
        Source::AlreadyEnabled => ModemClockError::AlreadyEnabled,
        Source::NotEnabled => ModemClockError::NotEnabled,
        Source::Rejected => ModemClockError::Rejected,
        Source::Poisoned => ModemClockError::Poisoned,
    }
}

pub(super) fn power_error(value: oer_chips_family_radio_power::power::PowerError) -> PowerError {
    let oer_chips_family_radio_power::power::PowerError {
        checkpoint,
        expected,
        observed,
    } = value;
    PowerError {
        checkpoint: power_checkpoint(checkpoint),
        expected,
        observed,
    }
}

pub(super) fn power_checkpoint(
    value: oer_chips_family_radio_power::power::PowerCheckpoint,
) -> PowerCheckpoint {
    use oer_chips_family_radio_power::power::PowerCheckpoint as Source;
    match value {
        Source::Mpll => PowerCheckpoint::Mpll,
        Source::ResetReleased => PowerCheckpoint::ResetReleased,
        Source::HpActiveIcg => PowerCheckpoint::HpActiveIcg,
        Source::ModemBusClock => PowerCheckpoint::ModemBusClock,
        Source::HpActiveClockMap => PowerCheckpoint::HpActiveClockMap,
        Source::SharedClockMap => PowerCheckpoint::SharedClockMap,
        Source::Reference160m => PowerCheckpoint::Reference160m,
        Source::ModemClockSource => PowerCheckpoint::ModemClockSource,
        Source::PhyClocks => PowerCheckpoint::PhyClocks,
        Source::I2cSource => PowerCheckpoint::I2cSource,
        Source::I2cClock => PowerCheckpoint::I2cClock,
    }
}

pub(super) fn power_restore_checkpoint(
    value: oer_chips_esp32s31_hal::system::root::WifiPowerRestoreCheckpoint,
) -> PowerRestoreCheckpoint {
    use oer_chips_esp32s31_hal::system::root::WifiPowerRestoreCheckpoint as Source;
    match value {
        Source::ModemSyscon => PowerRestoreCheckpoint::ModemSyscon,
        Source::ModemSourceClocks => PowerRestoreCheckpoint::ModemSourceClocks,
        Source::ModemRegisterBusClock => PowerRestoreCheckpoint::ModemRegisterBusClock,
    }
}

pub(super) fn platform_clock(value: oer_chips_esp32s31_hal::power::PlatformClock) -> PlatformClock {
    use oer_chips_esp32s31_hal::power::PlatformClock as Source;
    match value {
        Source::Pll160m => PlatformClock::Pll160m,
        Source::AnalogI2cMaster => PlatformClock::AnalogI2cMaster,
        Source::Mpll => PlatformClock::Mpll,
        Source::ModemCoexistence => PlatformClock::ModemCoexistence,
        Source::ModemLowPowerTimer => PlatformClock::ModemLowPowerTimer,
    }
}
pub(super) fn rx_block_ack_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::hardware::S31RxBlockAckAgreementError,
) -> RxBlockAckError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::hardware::S31RxBlockAckAgreementError as Source;
    match value {
        Source::HardwareIndex(v) => RxBlockAckError::HardwareIndex(v),
        Source::MulticastPeer => RxBlockAckError::MulticastPeer,
        Source::Tid(v) => RxBlockAckError::Tid(v),
        Source::Window(v) => RxBlockAckError::Window(v),
        Source::HardwareReadbackMismatch => RxBlockAckError::HardwareReadbackMismatch,
        Source::Domain => RxBlockAckError::Domain,
    }
}
pub(super) fn rx_stage_transaction_error(
    value: oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::pool::RxStageTransactionError,
) -> RxStageTransactionError {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::pool::RxStageTransactionError as Source;
    match value {
        Source::Stage(v) => RxStageTransactionError::Stage(rx_stage_error(v)),
        Source::Ring(v) => RxStageTransactionError::Ring(rx_ring_error(v)),
        Source::InvalidCreditReserve {
            reserved,
            pool_slots,
            queue_depth,
        } => RxStageTransactionError::InvalidCreditReserve {
            reserved: reserved as u64,
            pool_slots: pool_slots as u64,
            queue_depth: queue_depth as u64,
        },
    }
}

/// The answer of a release that left the actor and its domain as named.
pub(crate) fn released(
    domain: oer_chips_esp32s31_radio_client::DomainHealth,
) -> Result<(), ReleaseCause> {
    match domain {
        oer_chips_esp32s31_radio_client::DomainHealth::Healthy => Ok(()),
        oer_chips_esp32s31_radio_client::DomainHealth::Poisoned(poison) => {
            Err(ReleaseCause::DomainPoisoned(radio_poison(poison)))
        }
    }
}

pub(crate) fn still_active(
    error: oer_chips_esp32s31_hal::system::owner::maintenance::Error,
) -> ReleaseCause {
    ReleaseCause::StillActive(maintenance_error(error))
}

/// The terminal cause of a lifecycle's end; `None` for an ordinary stop
/// and a restart, which the facade serves itself.
#[cfg(target_arch = "riscv32")]
pub(crate) fn lifecycle(
    end: oer_protocols_ieee80211_runtime::lifecycle::LifecycleEnd<
        oer_chips_esp32s31_radio_ieee80211_backend::port::Fault,
    >,
) -> Option<LifecycleFailure> {
    use oer_protocols_ieee80211_runtime::datapath::ActorFault;
    use oer_protocols_ieee80211_runtime::lifecycle::LifecycleEnd as End;
    Some(match end {
        End::Stopped | End::StoppedReturning(_) | End::Restart(_) => return None,
        End::StartRefused(refused) => LifecycleFailure::StartRefused(start_refused(refused)),
        End::StopUnconfirmed(observed) => LifecycleFailure::StopUnconfirmed(observed),
        End::RetuneUnconfirmed(unconfirmed) => LifecycleFailure::RetuneUnconfirmed(unconfirmed),
        End::Faulted(ActorFault::Port(fault)) => LifecycleFailure::Port(backend_fault(fault)),
        End::Faulted(ActorFault::Contract(violation)) => LifecycleFailure::Actor(violation),
        End::Contract(violation) => LifecycleFailure::Contract(violation),
    })
}

/// The broken contract a restart of the station came from, if one did.
#[cfg(target_arch = "riscv32")]
pub(crate) fn restart(
    failure: oer_protocols_ieee80211_runtime::lifecycle::StationFailure<
        oer_chips_esp32s31_radio_ieee80211_backend::port::Fault,
    >,
) -> Option<LifecycleFailure> {
    use oer_protocols_ieee80211_runtime::datapath::ActorFault;
    use oer_protocols_ieee80211_runtime::lifecycle::StationFailure as Failure;
    match failure {
        Failure::Fault(ActorFault::Port(fault)) => {
            Some(LifecycleFailure::Port(backend_fault(fault)))
        }
        Failure::Fault(ActorFault::Contract(violation)) => Some(LifecycleFailure::Actor(violation)),
        _ => None,
    }
}

/// Why the port did not reinstall.
#[cfg(target_arch = "riscv32")]
pub(crate) fn reinstall(
    refused: oer_chips_esp32s31_radio_ieee80211_backend::port::ReinstallRefused,
) -> ReinstallCause {
    use oer_chips_esp32s31_radio_ieee80211_backend::port::ReinstallRefused as Source;
    match refused {
        Source::NotRetired => ReinstallCause::NotRetired,
        Source::SameEpoch => ReinstallCause::SameEpoch,
        Source::ChannelUnset => ReinstallCause::ChannelUnset,
        Source::Receipt => ReinstallCause::Receipt,
        Source::Resources => ReinstallCause::Resources,
    }
}
