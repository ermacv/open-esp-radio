//! The station's reasons and faults as HIL wire values. Every match is
//! exhaustive, so a new runtime or driver variant does not compile until it
//! has its wire value.
use oer_esp32s31_wifi::failure::*;
use oer_ieee80211_control::*;
pub fn service(fault: StationFailure) -> StationServiceFault {
    match fault {
        StationFailure::RxBlockAckCapacity => StationServiceFault::RxBlockAckCapacity,
        StationFailure::RxBlockAckEncode => StationServiceFault::RxBlockAckEncode,
        StationFailure::EnableRefused(v) => StationServiceFault::EnableRefused(backend(v)),
        StationFailure::Backend(v) => StationServiceFault::Backend(backend(v)),
        StationFailure::Submission(v) => StationServiceFault::Submission(backend(v)),
        StationFailure::Contract(v) => {
            StationServiceFault::Contract(oer_ieee80211_control::contract_violation(v))
        }
        StationFailure::DrainedWithOutstanding { cause, missing } => {
            StationServiceFault::DrainedWithOutstanding {
                cause: backend(cause),
                missing: oer_ieee80211_control::pending_terminal(missing),
            }
        }
    }
}

pub fn backend(fault: BackendFault) -> StationBackendFault {
    match fault {
        BackendFault::Core(core) => StationBackendFault::Core(match core {
            LowerMacFault::Tx(_) => StationLowerMacFault::Tx,
            LowerMacFault::RxBlockAckReadback => StationLowerMacFault::RxBlockAckReadback,
            LowerMacFault::Key(_) => StationLowerMacFault::Key,
            LowerMacFault::RxBlockAck(_) => StationLowerMacFault::RxBlockAck,
            LowerMacFault::NoRxBlockAckBank => StationLowerMacFault::NoRxBlockAckBank,
            LowerMacFault::Abandoned => StationLowerMacFault::Abandoned,
            LowerMacFault::KeyUseMismatch => StationLowerMacFault::KeyUseMismatch,
        }),
        BackendFault::Receive(cause) => StationBackendFault::Receive(match cause {
            ReceiveFault::StopRequested => StationReceiveFault::StopRequested,
            ReceiveFault::Prepare(_) => StationReceiveFault::Prepare,
            ReceiveFault::LineRefused => StationReceiveFault::LineRefused,
            ReceiveFault::Start(_) => StationReceiveFault::Start,
            ReceiveFault::Service(_) => StationReceiveFault::Service,
            ReceiveFault::ForeignExecutor => StationReceiveFault::ForeignExecutor,
        }),
        BackendFault::Deadline => StationBackendFault::Deadline,
        BackendFault::Capacity => StationBackendFault::Capacity,
        BackendFault::Clock => StationBackendFault::Clock,
        BackendFault::Retune(cause) => StationBackendFault::Retune(retune(cause)),
        BackendFault::Abandoned => StationBackendFault::Abandoned,
    }
}

fn retune(cause: RetuneError) -> StationRetuneFault {
    match cause {
        RetuneError::NotStopped(MaintenanceError::MacActive { state }) => {
            StationRetuneFault::MacActive { state }
        }
        RetuneError::NotStopped(MaintenanceError::RxWalkerEnabled) => {
            StationRetuneFault::RxWalkerEnabled
        }
        RetuneError::Radio(RadioFailure::Refused) => StationRetuneFault::Refused,
        RetuneError::Radio(RadioFailure::ClientFailed) => StationRetuneFault::ClientFailed,
        RetuneError::Radio(RadioFailure::Poisoned(poison)) => {
            StationRetuneFault::Poisoned(radio_poison(poison))
        }
    }
}

fn radio_poison(poison: RadioPoison) -> StationRadioPoison {
    use RadioPoisonCause as C;
    use StationRadioPoisonCause as W;
    StationRadioPoison {
        origin: match poison.origin {
            RadioOperation::TrackingTick => StationRadioOperation::TrackingTick,
            RadioOperation::Prepare => StationRadioOperation::Prepare,
            RadioOperation::CloseRf => StationRadioOperation::CloseRf,
            RadioOperation::Request { client, request } => StationRadioOperation::Request {
                client: match client {
                    RadioClient::Wifi => StationRadioClient::Wifi,
                    RadioClient::Bluetooth => StationRadioClient::Bluetooth,
                    RadioClient::Ieee802154 => StationRadioClient::Ieee802154,
                },
                request: match request {
                    RadioRequest::Join => StationRadioRequest::Join,
                    RadioRequest::Track => StationRadioRequest::Track,
                    RadioRequest::Retune => StationRadioRequest::Retune,
                    RadioRequest::Leave => StationRadioRequest::Leave,
                },
            },
        },
        cause: match poison.cause {
            C::TrackingRefused(error) => W::TrackingRefused(phy_domain(error)),
            C::Tracking(fault) => W::Tracking(phy(fault)),
            C::CalibrationTracking(fault) => W::CalibrationTracking(calibration_tracking(fault)),
            C::Channel(fault) => W::Channel(phy(fault)),
            C::Rf(fault) => W::Rf(phy(fault)),
            C::Registration(fault) => W::Registration(registration(fault)),
            C::Domain(error) => W::Domain(phy_domain(error)),
            C::CommonPower(_) => W::CommonPower,
            C::ModemClock(_) => W::ModemClock,
            C::Rollback => W::Rollback,
            C::Controller => W::Controller,
            C::Custody => W::Custody,
        },
    }
}

fn phy(fault: PhyFault) -> StationPhyFault {
    match fault {
        PhyFault::EdgeTimedOut => StationPhyFault::EdgeTimedOut,
        PhyFault::CapabilityUnavailable => StationPhyFault::CapabilityUnavailable,
        PhyFault::HardwareInvariant => StationPhyFault::HardwareInvariant,
        PhyFault::RfOperationLimit => StationPhyFault::RfOperationLimit,
        PhyFault::UnexpectedBinding => StationPhyFault::UnexpectedBinding,
        PhyFault::RegistrationEpochMismatch => StationPhyFault::RegistrationEpochMismatch,
        PhyFault::DeadlineOverflow => StationPhyFault::DeadlineOverflow,
        PhyFault::Sequence => StationPhyFault::Sequence,
    }
}

fn calibration_tracking(fault: CalibrationTrackingFault) -> StationCalibrationTrackingFault {
    use CalibrationTrackingFailure as F;
    use StationCalibrationTrackingFailure as W;
    match fault {
        CalibrationTrackingFault::Step(fault) => StationCalibrationTrackingFault::Step(phy(fault)),
        CalibrationTrackingFault::Radio(failure) => {
            StationCalibrationTrackingFault::Radio(match failure {
                F::PbusClearTimedOut => W::PbusClearTimedOut,
                F::Dcode => W::Dcode,
                F::RxGain => W::RxGain,
                F::Channel => W::Channel,
                F::TxDcPwdet => W::TxDcPwdet,
            })
        }
    }
}

fn registration(fault: RegistrationFault) -> StationRegistrationFault {
    use RegistrationFailure as F;
    use StationRegistrationFailure as W;
    match fault {
        RegistrationFault::Step(fault) => StationRegistrationFault::Step(phy(fault)),
        RegistrationFault::Lowering { stage, error } => StationRegistrationFault::Lowering {
            stage: match stage {
                PhyRegisterStage::Mmio => StationRegisterStage::Mmio,
                PhyRegisterStage::Delay => StationRegisterStage::Delay,
                PhyRegisterStage::I2cMasterResetSample => {
                    StationRegisterStage::I2cMasterResetSample
                }
                PhyRegisterStage::Rf => StationRegisterStage::Rf,
                PhyRegisterStage::Baseband => StationRegisterStage::Baseband,
                PhyRegisterStage::Temperature => StationRegisterStage::Temperature,
                PhyRegisterStage::FinalI2cRead => StationRegisterStage::FinalI2cRead,
            },
            error: match error {
                PhyRegisterBindingError::UnsupportedAction => {
                    StationRegisterBindingError::UnsupportedAction
                }
                PhyRegisterBindingError::IncompleteTransaction => {
                    StationRegisterBindingError::IncompleteTransaction
                }
                PhyRegisterBindingError::UnexpectedOutcome => {
                    StationRegisterBindingError::UnexpectedOutcome
                }
            },
        },
        RegistrationFault::Radio(failure) => StationRegistrationFault::Radio(match failure {
            F::I2cMasterResetTimedOut { index, samples } => {
                W::I2cMasterResetTimedOut { index, samples }
            }
            F::Rf => W::Rf,
            F::Baseband => W::Baseband,
            F::Temperature => W::Temperature,
            F::FinalI2cDeadlineExceeded => W::FinalI2cDeadlineExceeded,
        }),
    }
}

fn phy_domain(error: PhyDomainError) -> StationPhyDomainError {
    use PhyDomainError as E;
    use StationPhyDomainError as W;
    let time = |error| match error {
        PhyTrackTimeError::DeadlineOverflow { .. } => W::TimeOverflow,
        PhyTrackTimeError::TimeReversed {
            previous_micros,
            now_micros,
            ..
        } => W::TimeReversed {
            previous_us: previous_micros,
            now_us: now_micros,
        },
    };
    match error {
        E::NotRegistered => W::NotRegistered,
        E::AlreadyRegistered => W::AlreadyRegistered,
        E::TrackingPending => W::TrackingPending,
        E::NoTrackingPending => W::NoTrackingPending,
        E::Poisoned => W::Poisoned,
        E::RfClosed => W::RfClosed,
        E::RfOpen => W::RfOpen,
        E::EpochMismatch => W::EpochMismatch,
        E::ClientsActive => W::ClientsActive,
        E::Clock(_) => W::Clock,
        E::Acquire(PhyClientAcquireError::TrackingTime(error)) | E::Time(error) => time(error),
        E::ClientAbsent(_) => W::ClientAbsent,
    }
}
