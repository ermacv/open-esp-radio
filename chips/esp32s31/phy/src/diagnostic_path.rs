//! The variant paths of the PHY's errors, for diagnostics that do not
//! format. Each `match` is exhaustive: a new variant breaks the build.

use oer_diagnostics::{DiagPath, DiagnosticPath};
use oer_esp32s31_hal::domain::RadioClient;

use crate::calibration::registration::PhyCalibrationPath;
use crate::concurrent::{ConcurrentAcquire, ConcurrentPhyError, JoinRefused};
use crate::ieee802154_client::Ieee802154PhyClientError;
use crate::state::client::{PhyClientAcquireError, PhyPllTrackClass, PhyTrackTimeError};

impl PhyCalibrationPath {
    pub const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::FullUncached => oer_diagnostics::name!("FullUncached"; 32),
            Self::FullForCache => oer_diagnostics::name!("FullForCache"; 32),
            Self::FullAfterRejectedCache => oer_diagnostics::name!("FullAfterRejectedCache"; 32),
            Self::PartialFromCache => oer_diagnostics::name!("PartialFromCache"; 32),
        }
    }
}

impl DiagnosticPath for PhyCalibrationPath {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(self.name().as_str());
    }
}

impl DiagnosticPath for ConcurrentAcquire {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::Settled => "Settled",
            Self::TrackingDue => "TrackingDue",
        });
    }
}

impl DiagnosticPath for PhyPllTrackClass {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::Wifi => "Wifi",
            Self::BluetoothIeee802154 => "BluetoothIeee802154",
        });
    }
}

impl DiagnosticPath for PhyTrackTimeError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::DeadlineOverflow { class } => {
                path.name("DeadlineOverflow");
                class.push_path(path);
            }
            Self::TimeReversed {
                class,
                previous_micros,
                now_micros,
            } => {
                path.name("TimeReversed");
                class.push_path(path);
                path.wide(*previous_micros);
                path.wide(*now_micros);
            }
        }
    }
}

impl DiagnosticPath for PhyClientAcquireError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::TrackingTime(error) => {
                path.name("TrackingTime");
                error.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for ConcurrentPhyError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::NotRegistered => path.name("NotRegistered"),
            Self::AlreadyRegistered => path.name("AlreadyRegistered"),
            Self::TrackingPending => path.name("TrackingPending"),
            Self::NoTrackingPending => path.name("NoTrackingPending"),
            Self::Poisoned => path.name("Poisoned"),
            Self::RfClosed => path.name("RfClosed"),
            Self::RfOpen => path.name("RfOpen"),
            Self::EpochMismatch => path.name("EpochMismatch"),
            Self::ClientsActive => path.name("ClientsActive"),
            Self::Clock(error) => {
                path.name("Clock");
                error.push_path(path);
            }
            Self::Acquire(error) => {
                path.name("Acquire");
                error.push_path(path);
            }
            Self::Time(error) => {
                path.name("Time");
                error.push_path(path);
            }
            Self::ClientAbsent(client) => {
                path.name("ClientAbsent");
                path.name(match client {
                    RadioClient::Wifi => "Wifi",
                    RadioClient::Bluetooth => "Bluetooth",
                    RadioClient::Ieee802154 => "Ieee802154",
                });
            }
        }
    }
}

impl DiagnosticPath for Ieee802154PhyClientError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Phy(error) => {
                path.name("Phy");
                error.push_path(path);
            }
            Self::Btbb(error) => {
                path.name("Btbb");
                error.push_path(path);
            }
        }
    }
}

impl<B: oer_esp32s31_hal::power::PlatformClockBackend, E: DiagnosticPath> DiagnosticPath
    for JoinRefused<'_, B, E>
{
    /// The refusal's error; the radio it returns is not part of the path.
    fn push_path(&self, path: &mut DiagPath) {
        self.error.push_path(path);
    }
}

impl<E: DiagnosticPath> DiagnosticPath for crate::executor::PhyParamTrackingRunError<E> {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Port(error) => {
                path.name("Port");
                error.push_path(path);
            }
            Self::Transition(error) => {
                path.name("Transition");
                error.push_path(path);
            }
            Self::ParentEdgeLimit => path.name("ParentEdgeLimit"),
        }
    }
}

impl<E: DiagnosticPath> DiagnosticPath for crate::executor::PhyCalibrationTrackingRunError<E> {
    fn push_path(&self, path: &mut DiagPath) {
        use crate::tracking::calibration::{
            PhyCalibrationTrackingFailure as Failure,
            PhyCalibrationTrackingTransitionError as Transition,
        };
        match self {
            Self::Port(error) => {
                path.name("Port");
                error.push_path(path);
            }
            Self::Transition(error) => {
                path.name("Transition");
                path.name(match error {
                    Transition::WrongCompletion => "WrongCompletion",
                    Transition::AlreadyComplete => "AlreadyComplete",
                });
            }
            Self::Radio(failure) => {
                path.name("Radio");
                path.name(match failure {
                    Failure::PbusClearTimedOut(_) => "PbusClearTimedOut",
                    Failure::Dcode(_) => "Dcode",
                    Failure::RxGain(_) => "RxGain",
                    Failure::Channel(_) => "Channel",
                    Failure::TxDcPwdet(_) => "TxDcPwdet",
                });
            }
            Self::ParentEdgeLimit => path.name("ParentEdgeLimit"),
        }
    }
}

impl DiagnosticPath for crate::tracking::parameters::PhyParamTrackingTransitionError {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(match self {
            Self::WrongCompletion => "WrongCompletion",
            Self::AlreadyComplete => "AlreadyComplete",
        });
    }
}

#[cfg(test)]
mod tests {
    use oer_diagnostics::{DiagPath, DiagnosticPath};

    use crate::{executor::PhyCalibrationTrackingRunError, tracking::calibration};

    struct Leaf;

    impl DiagnosticPath for Leaf {
        fn push_path(&self, path: &mut DiagPath) {
            path.name("Leaf");
        }
    }

    #[test]
    fn a_calibration_tracking_error_names_its_own_step() {
        let port = PhyCalibrationTrackingRunError::Port(Leaf);
        assert_eq!(DiagPath::of(&port).names(), ["Port", "Leaf"]);
        let transition = PhyCalibrationTrackingRunError::<Leaf>::Transition(
            calibration::PhyCalibrationTrackingTransitionError::WrongCompletion,
        );
        assert_eq!(
            DiagPath::of(&transition).names(),
            ["Transition", "WrongCompletion"]
        );
        let limit = PhyCalibrationTrackingRunError::<Leaf>::ParentEdgeLimit;
        assert_eq!(DiagPath::of(&limit).names(), ["ParentEdgeLimit"]);
    }
}
