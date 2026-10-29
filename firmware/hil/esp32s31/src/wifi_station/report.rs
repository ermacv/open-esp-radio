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
use oer_ieee80211_arbiter::{LeaveReason, Refusal};
use oer_ieee80211_control::*;
use oer_ieee80211_port::{LayoutError, PreparationError};
use oer_ieee80211_rsn::sae::SaeError;
use oer_ieee80211_runtime::station::{
    BackendContractViolation, ControlTerminal, PendingTerminal, ServiceFault, StationPort,
};
use oer_ieee80211_sta::{
    join::sae::StaSaeFailure,
    machine::{StaDisconnectReason, StaKeyContractViolation, TxRefusal},
};
use oer_ieee80211_wire::station::StaDisconnectKind;
use oer_radio_port::PortBackend;

#[path = "security_report.rs"]
mod security_report;

pub fn disconnect(reason: StaDisconnectReason) -> StationDisconnectReason {
    use StaDisconnectReason as R;
    use StationDisconnectReason as W;
    match reason {
        R::InvalidEapol(error) => W::InvalidEapol(security_report::eapol(error)),
        R::RetainedSecurityIes(error) => {
            W::RetainedSecurityIes(security_report::retained_security_ies(error))
        }
        R::TransmitRefused(refusal) => W::TransmitRefused(transmit(refusal)),
        R::Requested => W::LocalRequest,
        R::NotFound { partial } => W::NotFound { partial },
        R::Refused(refusal) => W::Refused(channel(refusal)),
        R::LeaseExchangeRefused => W::LeaseExchangeRefused,
        R::Arbiter(leave) => W::Arbiter(match leave {
            LeaveReason::Stopped => StationLeaveReason::Stopped,
            LeaveReason::Unservable => StationLeaveReason::Unservable,
            LeaveReason::PortFailed => StationLeaveReason::PortFailed,
        }),
        R::AuthenticationFailed => W::AuthenticationFailed,
        R::Sae(failure) => W::Sae(sae(failure)),
        R::AssociationFailed => W::AssociationFailed,
        R::DeadlineOverflow => W::DeadlineOverflow,
        R::Peer { reason_code } => W::Peer { reason_code },
        R::Encode => W::Encode,
        R::HandshakeFailed => W::HandshakeFailed,
        R::SaQueryTimeout => W::SaQueryTimeout,
    }
}

fn transmit(refusal: TxRefusal) -> StationTransmitRefusal {
    match refusal {
        TxRefusal::PacketNumberExhausted => StationTransmitRefusal::PacketNumberExhausted,
        TxRefusal::LinkChanged => StationTransmitRefusal::LinkChanged,
        TxRefusal::NoRate => StationTransmitRefusal::NoRate,
        TxRefusal::Preparation(error) => StationTransmitRefusal::Preparation(match error {
            PreparationError::MissingBodyLoan => StationPreparationError::MissingBodyLoan,
            PreparationError::RetransmissionKey => StationPreparationError::RetransmissionKey,
            PreparationError::RetransmissionKind => StationPreparationError::RetransmissionKind,
            PreparationError::AggregateFull => StationPreparationError::AggregateFull,
            PreparationError::BlockAckWindow => StationPreparationError::BlockAckWindow,
        }),
        TxRefusal::Layout(error) => StationTransmitRefusal::Layout(match error {
            LayoutError::NumberingProtectionMismatch => {
                StationLayoutError::NumberingProtectionMismatch
            }
            LayoutError::StorageTooShort => StationLayoutError::StorageTooShort,
            LayoutError::CcmpHeaderTooShort => StationLayoutError::CcmpHeaderTooShort,
            LayoutError::BodyOverlap => StationLayoutError::BodyOverlap,
        }),
    }
}

fn channel(refusal: Refusal) -> StationChannelRefusal {
    match refusal {
        Refusal::IdentityExhausted => StationChannelRefusal::IdentityExhausted,
        Refusal::ScheduleExhausted => StationChannelRefusal::ScheduleExhausted,
        Refusal::Unservable => StationChannelRefusal::Unservable,
        Refusal::TooSoon { retry_at } => StationChannelRefusal::TooSoon {
            retry_at_us: retry_at.as_micros(),
        },
    }
}

fn sae(failure: StaSaeFailure) -> StationSaeFailure {
    match failure {
        StaSaeFailure::Crypto { peer, pwe } => StationSaeFailure::Crypto {
            peer,
            pwe: security_report::sae_pwe(pwe),
        },
        StaSaeFailure::Timeout => StationSaeFailure::Timeout,
        StaSaeFailure::DeadlineOverflow => StationSaeFailure::DeadlineOverflow,
        StaSaeFailure::PeerDisconnect(disconnect) => StationSaeFailure::PeerDisconnect {
            kind: match disconnect.kind {
                StaDisconnectKind::Disassociation => StationPeerDisconnectKind::Disassociation,
                StaDisconnectKind::Deauthentication => StationPeerDisconnectKind::Deauthentication,
            },
            reason_code: disconnect.reason_code,
        },
        StaSaeFailure::Rejected { status_code } => StationSaeFailure::Rejected { status_code },
        StaSaeFailure::Protocol(error) => StationSaeFailure::Protocol(match error {
            SaeError::Crypto => StationSaeError::Crypto,
            SaeError::NoPasswordElement => StationSaeError::NoPasswordElement,
            SaeError::UnsuitableRandom => StationSaeError::UnsuitableRandom,
            SaeError::UnsupportedGroup(group) => StationSaeError::UnsupportedGroup(group),
            SaeError::Malformed => StationSaeError::Malformed,
            SaeError::InvalidScalar => StationSaeError::InvalidScalar,
            SaeError::InvalidElement => StationSaeError::InvalidElement,
            SaeError::Reflection => StationSaeError::Reflection,
            SaeError::InfiniteSecret => StationSaeError::InfiniteSecret,
            SaeError::ConfirmMismatch => StationSaeError::ConfirmMismatch,
        }),
        StaSaeFailure::TokenTooLong => StationSaeFailure::TokenTooLong,
    }
}

pub fn service<B>(fault: ServiceFault<B>) -> StationServiceFault
where
    B: StationPort,
    B::Backend: PortBackend<B::SlotId, Fault = Fault, Refusal = Fault>,
{
    match fault {
        ServiceFault::RxBlockAckCapacity => StationServiceFault::RxBlockAckCapacity,
        ServiceFault::RxBlockAckEncode => StationServiceFault::RxBlockAckEncode,
        ServiceFault::EnableRefused(cause) => StationServiceFault::EnableRefused(backend(cause)),
        ServiceFault::Backend(cause) => StationServiceFault::Backend(backend(cause)),
        ServiceFault::Submission(cause) => StationServiceFault::Submission(backend(cause)),
        ServiceFault::Contract(violation) => StationServiceFault::Contract(contract(violation)),
        ServiceFault::DrainedWithOutstanding { cause, missing } => {
            StationServiceFault::DrainedWithOutstanding {
                cause: backend(cause),
                missing: pending(missing),
            }
        }
    }
}

fn contract(violation: BackendContractViolation) -> StationContractViolation {
    use BackendContractViolation as V;
    use StationContractViolation as W;
    match violation {
        V::UnexpectedAggregate => W::UnexpectedAggregate,
        V::UnexpectedTerminal(terminal) => W::UnexpectedTerminal(pending(terminal)),
        V::TerminalAfterEventEnd => W::TerminalAfterEventEnd,
        V::UnexpectedAccessPointTerminal => W::UnexpectedAccessPointTerminal,
        V::UnexpectedTsfSample => W::UnexpectedTsfSample,
        V::GroupTerminalWithoutRequest => W::GroupTerminalWithoutRequest,
        V::GroupSlotMismatch => W::GroupSlotMismatch,
        V::GroupTerminalKindMismatch => W::GroupTerminalKindMismatch,
        V::TxContextBufferMismatch => W::TxContextBufferMismatch,
        V::StationKeyBank(violation) => W::StationKeyBank(match violation {
            StaKeyContractViolation::IncompleteBank => StationKeyContractViolation::IncompleteBank,
            StaKeyContractViolation::MissingAssociation => {
                StationKeyContractViolation::MissingAssociation
            }
        }),
        V::StationKeyUnwrap => W::StationKeyUnwrap,
    }
}

fn pending(terminal: PendingTerminal) -> StationPendingTerminal {
    match terminal {
        PendingTerminal::Transmit(index) => StationPendingTerminal::Transmit(index),
        PendingTerminal::Control(control) => StationPendingTerminal::Control(match control {
            ControlTerminal::Configure => StationControlTerminal::Configure,
            ControlTerminal::PairwiseRemoval => StationControlTerminal::PairwiseRemoval,
            ControlTerminal::GroupChange => StationControlTerminal::GroupChange,
            ControlTerminal::GateClosed => StationControlTerminal::GateClosed,
        }),
    }
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
