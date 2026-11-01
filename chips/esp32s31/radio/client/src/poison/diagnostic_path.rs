//! The variant paths of a poison, for retained diagnostics that do not
//! format. Each `match` is exhaustive: a new variant breaks the build.

use oer_diagnostics::{DiagPath, DiagnosticPath};
use oer_esp32s31_hal::domain::{CommonRadioPowerError, RadioClient};
use oer_esp32s31_phy::{PhyRegisterBindingError, PhyRegisterStage};

use super::{
    CalibrationTrackingFailure, CalibrationTrackingFault, PhyFault, Poison, PoisonCause,
    PoisonOrigin, RegistrationFailure, RegistrationFault, RequestKind,
};

impl DiagnosticPath for Poison {
    fn push_path(&self, path: &mut DiagPath) {
        self.origin.push_path(path);
        self.cause.push_path(path);
    }
}

impl DiagnosticPath for PoisonOrigin {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::TrackingTick => path.name("TrackingTick"),
            Self::Prepare => path.name("Prepare"),
            Self::CloseRf => path.name("CloseRf"),
            Self::Request { client, request } => {
                path.name("Request");
                path.name(match client {
                    RadioClient::Wifi => "Wifi",
                    RadioClient::Bluetooth => "Bluetooth",
                    RadioClient::Ieee802154 => "Ieee802154",
                });
                request.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for RequestKind {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::Join => "Join",
            Self::Track => "Track",
            Self::Retune => "Retune",
            Self::Leave => "Leave",
        });
    }
}

impl DiagnosticPath for PoisonCause {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::TrackingRefused(error) => {
                path.name("TrackingRefused");
                error.push_path(path);
            }
            Self::Tracking(fault) => {
                path.name("Tracking");
                fault.push_path(path);
            }
            Self::CalibrationTracking(fault) => {
                path.name("CalibrationTracking");
                fault.push_path(path);
            }
            Self::Channel(fault) => {
                path.name("Channel");
                fault.push_path(path);
            }
            Self::Rf(fault) => {
                path.name("Rf");
                fault.push_path(path);
            }
            Self::Registration(fault) => {
                path.name("Registration");
                fault.push_path(path);
            }
            Self::Domain(error) => {
                path.name("Domain");
                error.push_path(path);
            }
            Self::CommonPower(error) => {
                path.name("CommonPower");
                match error {
                    CommonRadioPowerError::AlreadyEntered => path.name("AlreadyEntered"),
                    // The checkpoint's payload stays with the typed error;
                    // the path says it kept less.
                    CommonRadioPowerError::Power(_) => {
                        path.name("Power");
                        path.truncated();
                    }
                    CommonRadioPowerError::Restore(_) => {
                        path.name("Restore");
                        path.truncated();
                    }
                    CommonRadioPowerError::Release(_) => {
                        path.name("Release");
                        path.truncated();
                    }
                }
            }
            Self::ModemClock(error) => {
                path.name("ModemClock");
                error.push_path(path);
            }
            Self::Rollback => path.name("Rollback"),
            Self::Controller => path.name("Controller"),
            Self::Custody => path.name("Custody"),
        }
    }
}

impl DiagnosticPath for PhyFault {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::EdgeTimedOut => "EdgeTimedOut",
            Self::CapabilityUnavailable => "CapabilityUnavailable",
            Self::HardwareInvariant => "HardwareInvariant",
            Self::RfOperationLimit => "RfOperationLimit",
            Self::UnexpectedBinding => "UnexpectedBinding",
            Self::RegistrationEpochMismatch => "RegistrationEpochMismatch",
            Self::DeadlineOverflow => "DeadlineOverflow",
            Self::Sequence => "Sequence",
        });
    }
}

impl DiagnosticPath for CalibrationTrackingFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Step(fault) => {
                path.name("Step");
                fault.push_path(path);
            }
            Self::Radio(failure) => {
                path.name("Radio");
                failure.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for CalibrationTrackingFailure {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::PbusClearTimedOut => "PbusClearTimedOut",
            Self::Dcode => "Dcode",
            Self::RxGain => "RxGain",
            Self::Channel => "Channel",
            Self::TxDcPwdet => "TxDcPwdet",
        });
    }
}

impl DiagnosticPath for RegistrationFault {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Step(fault) => {
                path.name("Step");
                fault.push_path(path);
            }
            Self::Lowering { stage, error } => {
                path.name("Lowering");
                path.name(match stage {
                    PhyRegisterStage::Mmio => "Mmio",
                    PhyRegisterStage::Delay => "Delay",
                    PhyRegisterStage::I2cMasterResetSample => "I2cMasterResetSample",
                    PhyRegisterStage::Rf => "Rf",
                    PhyRegisterStage::Baseband => "Baseband",
                    PhyRegisterStage::Temperature => "Temperature",
                    PhyRegisterStage::FinalI2cRead => "FinalI2cRead",
                });
                path.name(match error {
                    PhyRegisterBindingError::UnsupportedAction => "UnsupportedAction",
                    PhyRegisterBindingError::IncompleteTransaction => "IncompleteTransaction",
                    PhyRegisterBindingError::UnexpectedOutcome => "UnexpectedOutcome",
                });
            }
            Self::Radio(failure) => {
                path.name("Radio");
                failure.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for RegistrationFailure {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::I2cMasterResetTimedOut { index, samples } => {
                path.name("I2cMasterResetTimedOut");
                path.word(u32::from(*index));
                path.word(u32::from(*samples));
            }
            Self::Rf => path.name("Rf"),
            Self::Baseband => path.name("Baseband"),
            Self::Temperature => path.name("Temperature"),
            Self::FinalI2cDeadlineExceeded => path.name("FinalI2cDeadlineExceeded"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_phy::concurrent::ConcurrentPhyError;
    use oer_esp32s31_phy::state::client::{PhyPllTrackClass, PhyTrackTimeError};

    #[test]
    fn a_tracking_poison_names_its_whole_path() {
        let poison = Poison {
            origin: PoisonOrigin::TrackingTick,
            cause: PoisonCause::CalibrationTracking(CalibrationTrackingFault::Radio(
                CalibrationTrackingFailure::TxDcPwdet,
            )),
        };
        let path = DiagPath::of(&poison);
        assert_eq!(
            path.names(),
            ["TrackingTick", "CalibrationTracking", "Radio", "TxDcPwdet"]
        );
        assert!(path.words().is_empty() && !path.overflowed());

        let overflow = PoisonCause::TrackingRefused(ConcurrentPhyError::Time(
            PhyTrackTimeError::DeadlineOverflow {
                class: PhyPllTrackClass::Wifi,
            },
        ));
        assert_eq!(
            DiagPath::of(&overflow).names(),
            ["TrackingRefused", "Time", "DeadlineOverflow", "Wifi"]
        );
    }

    #[test]
    fn numbers_stay_with_their_variant_and_dropped_payloads_say_so() {
        let path = DiagPath::of(&PoisonCause::Registration(RegistrationFault::Radio(
            RegistrationFailure::I2cMasterResetTimedOut {
                index: 3,
                samples: 700,
            },
        )));
        assert_eq!(
            path.names(),
            ["Registration", "Radio", "I2cMasterResetTimedOut"]
        );
        assert_eq!(path.words(), [3, 700]);
        let release = DiagPath::of(&PoisonCause::CommonPower(CommonRadioPowerError::Release(
            oer_esp32s31_hal::power::PlatformClock::Mpll,
        )));
        assert_eq!(release.names(), ["CommonPower", "Release"]);
        assert!(release.overflowed());
    }
}
