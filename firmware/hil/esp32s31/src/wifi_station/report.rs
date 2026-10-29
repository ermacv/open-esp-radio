//! The station's reasons and faults as HIL wire values. Every match is
//! exhaustive, so a new runtime or driver variant does not compile until it
//! has its wire value.
use oer_esp32s31_hal::{domain::RadioClient, owner::maintenance};
use oer_esp32s31_ieee80211_driver::{
    lower_mac::LowerMacFault,
    port::{Fault, WifiRetuneError},
    rx::control::WifiRxCause,
};
use oer_esp32s31_phy::{
    PhyRegisterBindingError, PhyRegisterStage,
    concurrent::ConcurrentPhyError,
    state::client::{PhyClientAcquireError, PhyTrackTimeError},
};
use oer_esp32s31_radio_client::{
    CalibrationTrackingFailure, CalibrationTrackingFault, Failure, PhyFault, Poison, PoisonCause,
    PoisonOrigin, RegistrationFailure, RegistrationFault, RequestKind,
};
use oer_ieee80211_control::*;
use oer_ieee80211_runtime::station::{ServiceFault, StationPort};
use oer_radio_port::PortBackend;

pub fn service<B>(fault: ServiceFault<B>) -> StationServiceFault
where
    B: StationPort,
    B::Backend: PortBackend<B::SlotId, Fault = Fault, Refusal = Fault>,
{
    service_fault(fault, backend, backend)
}

pub fn backend(fault: Fault) -> StationBackendFault {
    match fault {
        Fault::Core(core) => StationBackendFault::Core(match core {
            LowerMacFault::Tx(_) => StationLowerMacFault::Tx,
            LowerMacFault::RxBlockAckReadback => StationLowerMacFault::RxBlockAckReadback,
            LowerMacFault::Key(_) => StationLowerMacFault::Key,
            LowerMacFault::RxBlockAck(_) => StationLowerMacFault::RxBlockAck,
            LowerMacFault::NoRxBlockAckBank => StationLowerMacFault::NoRxBlockAckBank,
            LowerMacFault::Abandoned => StationLowerMacFault::Abandoned,
            LowerMacFault::KeyUseMismatch => StationLowerMacFault::KeyUseMismatch,
        }),
        Fault::Receive(cause) => StationBackendFault::Receive(match cause {
            WifiRxCause::StopRequested => StationReceiveFault::StopRequested,
            WifiRxCause::Prepare(_) => StationReceiveFault::Prepare,
            WifiRxCause::LineRefused => StationReceiveFault::LineRefused,
            WifiRxCause::Start(_) => StationReceiveFault::Start,
            WifiRxCause::Service(_) => StationReceiveFault::Service,
            WifiRxCause::ForeignExecutor => StationReceiveFault::ForeignExecutor,
        }),
        Fault::Deadline => StationBackendFault::Deadline,
        Fault::Capacity => StationBackendFault::Capacity,
        Fault::Clock => StationBackendFault::Clock,
        Fault::Retune(cause) => StationBackendFault::Retune(retune(cause)),
        Fault::Abandoned => StationBackendFault::Abandoned,
    }
}

fn retune(cause: WifiRetuneError) -> StationRetuneFault {
    match cause {
        WifiRetuneError::NotStopped(maintenance::Error::MacActive { state }) => {
            StationRetuneFault::MacActive { state }
        }
        WifiRetuneError::NotStopped(maintenance::Error::RxWalkerEnabled) => {
            StationRetuneFault::RxWalkerEnabled
        }
        WifiRetuneError::Radio(Failure::Refused) => StationRetuneFault::Refused,
        WifiRetuneError::Radio(Failure::ClientFailed) => StationRetuneFault::ClientFailed,
        WifiRetuneError::Radio(Failure::Poisoned(poison)) => {
            StationRetuneFault::Poisoned(radio_poison(poison))
        }
    }
}

fn radio_poison(poison: Poison) -> StationRadioPoison {
    use PoisonCause as C;
    use StationRadioPoisonCause as W;
    StationRadioPoison {
        origin: match poison.origin {
            PoisonOrigin::TrackingTick => StationRadioOperation::TrackingTick,
            PoisonOrigin::Prepare => StationRadioOperation::Prepare,
            PoisonOrigin::CloseRf => StationRadioOperation::CloseRf,
            PoisonOrigin::Request { client, request } => StationRadioOperation::Request {
                client: match client {
                    RadioClient::Wifi => StationRadioClient::Wifi,
                    RadioClient::Bluetooth => StationRadioClient::Bluetooth,
                    RadioClient::Ieee802154 => StationRadioClient::Ieee802154,
                },
                request: match request {
                    RequestKind::Join => StationRadioRequest::Join,
                    RequestKind::Track => StationRadioRequest::Track,
                    RequestKind::Retune => StationRadioRequest::Retune,
                    RequestKind::Leave => StationRadioRequest::Leave,
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

fn phy_domain(error: ConcurrentPhyError) -> StationPhyDomainError {
    use ConcurrentPhyError as E;
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
