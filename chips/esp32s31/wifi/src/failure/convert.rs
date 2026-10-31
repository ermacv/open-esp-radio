//! Exhaustive conversion at the facade's target boundary.
use super::*;
#[cfg(target_arch = "riscv32")]
use oer_ieee80211_runtime::station::{ServiceFault, StationPort};
#[cfg(target_arch = "riscv32")]
use oer_radio_port::PortBackend;
#[cfg(target_arch = "riscv32")]
pub(crate) fn service<B: StationPort>(value: ServiceFault<B>) -> StationFailure
where
    B::Backend: PortBackend<
            B::SlotId,
            Fault = oer_esp32s31_ieee80211_driver::port::Fault,
            Refusal = oer_esp32s31_ieee80211_driver::port::Fault,
        >,
{
    match value {
        ServiceFault::PolicyDeadlineOverflow => StationFailure::PolicyDeadlineOverflow,
        ServiceFault::CommandAdmission(request) => StationFailure::CommandAdmission(request),
        ServiceFault::RxBlockAckCapacity => StationFailure::RxBlockAckCapacity,
        ServiceFault::RxBlockAckEncode => StationFailure::RxBlockAckEncode,
        ServiceFault::EnableRefused(v) => StationFailure::EnableRefused(backend_fault(v)),
        ServiceFault::Backend(v) => StationFailure::Backend(backend_fault(v)),
        ServiceFault::Submission(v) => StationFailure::Submission(backend_fault(v)),
        ServiceFault::Contract(v) => StationFailure::Contract(v),
        ServiceFault::DrainedWithOutstanding { cause, missing } => {
            StationFailure::DrainedWithOutstanding {
                cause: backend_fault(cause),
                missing,
            }
        }
    }
}

pub(super) fn backend_fault(value: oer_esp32s31_ieee80211_driver::port::Fault) -> BackendFault {
    use oer_esp32s31_ieee80211_driver::port::Fault as Source;
    match value {
        Source::Core(v0) => BackendFault::Core(lower_mac_fault(v0)),
        Source::Receive(v0) => BackendFault::Receive(receive_fault(v0)),
        Source::Deadline => BackendFault::Deadline,
        Source::Capacity => BackendFault::Capacity,
        Source::Clock => BackendFault::Clock,
        Source::Retune(v0) => BackendFault::Retune(retune_error(v0)),
        Source::Abandoned => BackendFault::Abandoned,
        Source::UnexpectedAggregate => BackendFault::UnexpectedAggregate,
        Source::TuneNotRecorded => BackendFault::TuneNotRecorded,
        Source::Setting(v0) => BackendFault::Setting(setting_fault(v0)),
    }
}

pub(super) fn setting_fault(
    value: oer_esp32s31_ieee80211_driver::port::SettingFault,
) -> SettingFault {
    use oer_esp32s31_ieee80211_driver::port::SettingFault as Source;
    match value {
        Source::UnsupportedChannel => SettingFault::UnsupportedChannel,
        Source::ChannelUnset => SettingFault::ChannelUnset,
        Source::EdcaLifetime => SettingFault::EdcaLifetime,
        Source::BlockAckSequence => SettingFault::BlockAckSequence,
    }
}

pub(super) fn lower_mac_fault(
    value: oer_esp32s31_ieee80211_driver::lower_mac::LowerMacFault,
) -> LowerMacFault {
    use oer_esp32s31_ieee80211_driver::lower_mac::LowerMacFault as Source;
    match value {
        Source::Tx(v0) => LowerMacFault::Tx(ordinary_tx_error(v0)),
        Source::RxBlockAckReadback => LowerMacFault::RxBlockAckReadback,
        Source::Key(v0) => LowerMacFault::Key(crypto_key_error(v0)),
        Source::RxBlockAck(v0) => LowerMacFault::RxBlockAck(rx_block_ack_error(v0)),
        Source::NoRxBlockAckBank => LowerMacFault::NoRxBlockAckBank,
        Source::Abandoned => LowerMacFault::Abandoned,
        Source::KeyUseMismatch => LowerMacFault::KeyUseMismatch,
    }
}

pub(super) fn ordinary_tx_error(
    value: oer_esp32s31_ieee80211_driver::ordinary_tx::OrdinaryTxError,
) -> OrdinaryTxError {
    use oer_esp32s31_ieee80211_driver::ordinary_tx::OrdinaryTxError as Source;
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

pub(super) fn tx_error(value: oer_esp32s31_ieee80211_driver::mac::tx::TxError) -> TxError {
    use oer_esp32s31_ieee80211_driver::mac::tx::TxError as Source;
    match value {
        Source::Invalid => TxError::Invalid,
        Source::QueueActive => TxError::QueueActive,
        Source::Stale => TxError::Stale,
        Source::DetachFailed => TxError::DetachFailed,
        Source::TimeoutNotPending => TxError::TimeoutNotPending,
        Source::ResetRequired => TxError::ResetRequired,
        Source::Completion(v0) => TxError::Completion(completion_fault(v0)),
    }
}

pub(super) fn completion_fault(
    value: oer_esp32s31_ieee80211_driver::mac::tx::CompletionFault,
) -> CompletionFault {
    use oer_esp32s31_ieee80211_driver::mac::tx::CompletionFault as Source;
    match value {
        Source::SecurityKeyError => CompletionFault::SecurityKeyError,
        Source::InvalidStatus { status } => CompletionFault::InvalidStatus { status },
    }
}

pub(super) fn tx_reset_reason(
    value: oer_esp32s31_ieee80211_driver::ordinary_tx::TxResetReason,
) -> TxResetReason {
    use oer_esp32s31_ieee80211_driver::ordinary_tx::TxResetReason as Source;
    match value {
        Source::CompletionInterruptWithoutState => TxResetReason::CompletionInterruptWithoutState,
        Source::TimeoutInterruptWithoutState => TxResetReason::TimeoutInterruptWithoutState,
        Source::ConflictingInterruptEvents(v0) => TxResetReason::ConflictingInterruptEvents(v0),
        Source::ExecutorDeadline => TxResetReason::ExecutorDeadline,
    }
}

pub(super) fn crypto_key_error(
    value: oer_esp32s31_ieee80211_driver::mac::crypto::CryptoKeyError,
) -> CryptoKeyError {
    use oer_esp32s31_ieee80211_driver::mac::crypto::CryptoKeyError as Source;
    match value {
        Source::InvalidGroupKeyId => CryptoKeyError::InvalidGroupKeyId,
        Source::InvalidAccessPointAssociationId => CryptoKeyError::InvalidAccessPointAssociationId,
        Source::Occupied => CryptoKeyError::Occupied,
        Source::HardwareRejected => CryptoKeyError::HardwareRejected,
    }
}

pub(super) fn receive_fault(
    value: oer_esp32s31_ieee80211_driver::rx::control::WifiRxCause,
) -> ReceiveFault {
    use oer_esp32s31_ieee80211_driver::rx::control::WifiRxCause as Source;
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
    value: oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingError,
) -> RxRingError {
    use oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingError as Source;
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
    value: oer_esp32s31_ieee80211_driver::rx::frontier::RxFrontierError,
) -> RxFrontierError {
    use oer_esp32s31_ieee80211_driver::rx::frontier::RxFrontierError as Source;
    match value {
        Source::AlreadyStarted => RxFrontierError::AlreadyStarted,
        Source::OwnerUnavailable => RxFrontierError::OwnerUnavailable,
        Source::Ring(v0) => RxFrontierError::Ring(rx_ring_error(v0)),
    }
}

pub(super) fn rx_stage_error(
    value: oer_esp32s31_ieee80211_driver::mac::rx::pool::RxStageError,
) -> RxStageError {
    use oer_esp32s31_ieee80211_driver::mac::rx::pool::RxStageError as Source;
    match value {
        Source::Exhausted => RxStageError::Exhausted,
        Source::Empty => RxStageError::Empty,
        Source::TooLong => RxStageError::TooLong,
        Source::Chained => RxStageError::Chained,
    }
}

pub(super) fn retune_error(
    value: oer_esp32s31_ieee80211_driver::port::WifiRetuneError,
) -> RetuneError {
    use oer_esp32s31_ieee80211_driver::port::WifiRetuneError as Source;
    match value {
        Source::NotStopped(v0) => RetuneError::NotStopped(maintenance_error(v0)),
        Source::Radio(v0) => RetuneError::Radio(radio_failure(v0)),
    }
}

pub(super) fn maintenance_error(
    value: oer_esp32s31_hal::owner::maintenance::Error,
) -> MaintenanceError {
    use oer_esp32s31_hal::owner::maintenance::Error as Source;
    match value {
        Source::MacActive { state } => MaintenanceError::MacActive { state },
        Source::RxWalkerEnabled => MaintenanceError::RxWalkerEnabled,
    }
}

pub(super) fn radio_failure(value: oer_esp32s31_radio_client::Failure) -> RadioFailure {
    use oer_esp32s31_radio_client::Failure as Source;
    match value {
        Source::Refused => RadioFailure::Refused,
        Source::ClientFailed => RadioFailure::ClientFailed,
        Source::Poisoned(v0) => RadioFailure::Poisoned(radio_poison(v0)),
    }
}

pub(super) fn radio_poison(value: oer_esp32s31_radio_client::Poison) -> RadioPoison {
    let oer_esp32s31_radio_client::Poison { origin, cause } = value;
    RadioPoison {
        origin: radio_operation(origin),
        cause: radio_poison_cause(cause),
    }
}

pub(super) fn radio_operation(value: oer_esp32s31_radio_client::PoisonOrigin) -> RadioOperation {
    use oer_esp32s31_radio_client::PoisonOrigin as Source;
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

pub(super) fn radio_request(value: oer_esp32s31_radio_client::RequestKind) -> RadioRequest {
    use oer_esp32s31_radio_client::RequestKind as Source;
    match value {
        Source::Join => RadioRequest::Join,
        Source::Track => RadioRequest::Track,
        Source::Retune => RadioRequest::Retune,
        Source::Leave => RadioRequest::Leave,
    }
}

pub(super) fn radio_client(value: oer_esp32s31_hal::domain::RadioClient) -> RadioClient {
    use oer_esp32s31_hal::domain::RadioClient as Source;
    match value {
        Source::Wifi => RadioClient::Wifi,
        Source::Bluetooth => RadioClient::Bluetooth,
        Source::Ieee802154 => RadioClient::Ieee802154,
    }
}

pub(super) fn radio_poison_cause(
    value: oer_esp32s31_radio_client::PoisonCause,
) -> RadioPoisonCause {
    use oer_esp32s31_radio_client::PoisonCause as Source;
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
        Source::Controller => RadioPoisonCause::Controller,
        Source::Custody => RadioPoisonCause::Custody,
    }
}

pub(super) fn phy_fault(value: oer_esp32s31_radio_client::PhyFault) -> PhyFault {
    use oer_esp32s31_radio_client::PhyFault as Source;
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
    value: oer_esp32s31_radio_client::CalibrationTrackingFault,
) -> CalibrationTrackingFault {
    use oer_esp32s31_radio_client::CalibrationTrackingFault as Source;
    match value {
        Source::Step(v0) => CalibrationTrackingFault::Step(phy_fault(v0)),
        Source::Radio(v0) => CalibrationTrackingFault::Radio(calibration_tracking_failure(v0)),
    }
}

pub(super) fn calibration_tracking_failure(
    value: oer_esp32s31_radio_client::CalibrationTrackingFailure,
) -> CalibrationTrackingFailure {
    use oer_esp32s31_radio_client::CalibrationTrackingFailure as Source;
    match value {
        Source::PbusClearTimedOut => CalibrationTrackingFailure::PbusClearTimedOut,
        Source::Dcode => CalibrationTrackingFailure::Dcode,
        Source::RxGain => CalibrationTrackingFailure::RxGain,
        Source::Channel => CalibrationTrackingFailure::Channel,
        Source::TxDcPwdet => CalibrationTrackingFailure::TxDcPwdet,
    }
}

pub(super) fn registration_fault(
    value: oer_esp32s31_radio_client::RegistrationFault,
) -> RegistrationFault {
    use oer_esp32s31_radio_client::RegistrationFault as Source;
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
    value: oer_esp32s31_radio_client::RegistrationFailure,
) -> RegistrationFailure {
    use oer_esp32s31_radio_client::RegistrationFailure as Source;
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

pub(super) fn phy_register_stage(value: oer_esp32s31_phy::PhyRegisterStage) -> PhyRegisterStage {
    use oer_esp32s31_phy::PhyRegisterStage as Source;
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
    value: oer_esp32s31_phy::PhyRegisterBindingError,
) -> PhyRegisterBindingError {
    use oer_esp32s31_phy::PhyRegisterBindingError as Source;
    match value {
        Source::UnsupportedAction => PhyRegisterBindingError::UnsupportedAction,
        Source::IncompleteTransaction => PhyRegisterBindingError::IncompleteTransaction,
        Source::UnexpectedOutcome => PhyRegisterBindingError::UnexpectedOutcome,
    }
}

pub(super) fn phy_domain_error(
    value: oer_esp32s31_phy::concurrent::ConcurrentPhyError,
) -> PhyDomainError {
    use oer_esp32s31_phy::concurrent::ConcurrentPhyError as Source;
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
    }
}

pub(super) fn phy_track_time_error(
    value: oer_esp32s31_phy::state::client::PhyTrackTimeError,
) -> PhyTrackTimeError {
    use oer_esp32s31_phy::state::client::PhyTrackTimeError as Source;
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
    value: oer_esp32s31_phy::state::client::PhyClientAcquireError,
) -> PhyClientAcquireError {
    use oer_esp32s31_phy::state::client::PhyClientAcquireError as Source;
    match value {
        Source::TrackingTime(v0) => PhyClientAcquireError::TrackingTime(phy_track_time_error(v0)),
    }
}

pub(super) fn phy_pll_track_class(
    value: oer_esp32s31_phy::state::client::PhyPllTrackClass,
) -> PhyPllTrackClass {
    use oer_esp32s31_phy::state::client::PhyPllTrackClass as Source;
    match value {
        Source::Wifi => PhyPllTrackClass::Wifi,
        Source::BluetoothIeee802154 => PhyPllTrackClass::BluetoothIeee802154,
    }
}

pub(super) fn common_radio_power_error(
    value: oer_esp32s31_hal::domain::CommonRadioPowerError,
) -> CommonRadioPowerError {
    use oer_esp32s31_hal::domain::CommonRadioPowerError as Source;
    match value {
        Source::AlreadyEntered => CommonRadioPowerError::AlreadyEntered,
        Source::Power(v0) => CommonRadioPowerError::Power(power_error(v0)),
        Source::Restore(v0) => CommonRadioPowerError::Restore(power_restore_checkpoint(v0)),
        Source::Release(v0) => CommonRadioPowerError::Release(platform_clock(v0)),
    }
}

pub(super) fn modem_clock_error(
    value: oer_esp32s31_hal::domain::ModemClockError,
) -> ModemClockError {
    use oer_esp32s31_hal::domain::ModemClockError as Source;
    match value {
        Source::AlreadyEnabled => ModemClockError::AlreadyEnabled,
        Source::NotEnabled => ModemClockError::NotEnabled,
        Source::Rejected => ModemClockError::Rejected,
        Source::Poisoned => ModemClockError::Poisoned,
    }
}

pub(super) fn power_error(value: oer_espressif_radio::power::PowerError) -> PowerError {
    let oer_espressif_radio::power::PowerError {
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
    value: oer_espressif_radio::power::PowerCheckpoint,
) -> PowerCheckpoint {
    use oer_espressif_radio::power::PowerCheckpoint as Source;
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
    value: oer_esp32s31_hal::root::WifiPowerRestoreCheckpoint,
) -> PowerRestoreCheckpoint {
    use oer_esp32s31_hal::root::WifiPowerRestoreCheckpoint as Source;
    match value {
        Source::ModemSyscon => PowerRestoreCheckpoint::ModemSyscon,
        Source::ModemSourceClocks => PowerRestoreCheckpoint::ModemSourceClocks,
        Source::ModemRegisterBusClock => PowerRestoreCheckpoint::ModemRegisterBusClock,
    }
}

pub(super) fn platform_clock(value: oer_esp32s31_hal::power::PlatformClock) -> PlatformClock {
    use oer_esp32s31_hal::power::PlatformClock as Source;
    match value {
        Source::Pll160m => PlatformClock::Pll160m,
        Source::AnalogI2cMaster => PlatformClock::AnalogI2cMaster,
        Source::Mpll => PlatformClock::Mpll,
        Source::ModemCoexistence => PlatformClock::ModemCoexistence,
        Source::ModemLowPowerTimer => PlatformClock::ModemLowPowerTimer,
    }
}
pub(super) fn rx_block_ack_error(
    value: oer_esp32s31_ieee80211_driver::mac::rx::hardware::S31RxBlockAckAgreementError,
) -> RxBlockAckError {
    use oer_esp32s31_ieee80211_driver::mac::rx::hardware::S31RxBlockAckAgreementError as Source;
    match value {
        Source::HardwareIndex(v) => RxBlockAckError::HardwareIndex(v),
        Source::MulticastPeer => RxBlockAckError::MulticastPeer,
        Source::Tid(v) => RxBlockAckError::Tid(v),
        Source::Window(v) => RxBlockAckError::Window(v),
        Source::HardwareReadbackMismatch => RxBlockAckError::HardwareReadbackMismatch,
    }
}
pub(super) fn rx_stage_transaction_error(
    value: oer_esp32s31_ieee80211_driver::mac::rx::pool::RxStageTransactionError,
) -> RxStageTransactionError {
    use oer_esp32s31_ieee80211_driver::mac::rx::pool::RxStageTransactionError as Source;
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

pub(crate) fn retirement(
    failure: Option<StationFailure>,
    cause: oer_radio_port::LifecycleError,
    hardware_stopped: bool,
) -> RetirementFailure {
    RetirementFailure {
        failure,
        cause: retirement_cause(cause),
        hardware_stopped,
    }
}

pub(crate) fn retirement_cause(cause: oer_radio_port::LifecycleError) -> RetirementCause {
    use oer_radio_port::{LifecycleError as Source, SlotStorageState as Storage};
    match cause {
        Source::Cancelled => RetirementCause::Cancelled,
        Source::FailStopRequested => RetirementCause::FailStopRequested,
        Source::Construction => RetirementCause::Construction,
        Source::BodyAlreadyIssued => RetirementCause::BodyAlreadyIssued,
        Source::Deadline => RetirementCause::Deadline,
        Source::StopUnconfirmed => RetirementCause::StopUnconfirmed,
        Source::InvalidState(observed) => RetirementCause::InvalidState(SlotObservation {
            phase: observed.phase,
            storage: match observed.storage {
                Storage::Empty => SlotStorage::Empty,
                Storage::Registered => SlotStorage::Registered,
                Storage::Vacant => SlotStorage::Vacant,
                Storage::Unavailable => SlotStorage::Unavailable,
            },
            invocation_present: observed.invocation_present,
        }),
    }
}

/// The answer of a release that left the actor and its domain as named.
pub(crate) fn released(
    domain: oer_esp32s31_radio_client::DomainHealth,
) -> Result<(), ReleaseCause> {
    match domain {
        oer_esp32s31_radio_client::DomainHealth::Healthy => Ok(()),
        oer_esp32s31_radio_client::DomainHealth::Poisoned(poison) => {
            Err(ReleaseCause::DomainPoisoned(radio_poison(poison)))
        }
    }
}

pub(crate) fn still_active(error: oer_esp32s31_hal::owner::maintenance::Error) -> ReleaseCause {
    ReleaseCause::StillActive(maintenance_error(error))
}
