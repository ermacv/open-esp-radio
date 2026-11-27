//! The variant paths of the PHY's errors, for diagnostics that do not
//! format. Each `match` is exhaustive: a new variant breaks the build.

use oer_portable_diagnostics::{DiagPath, DiagnosticPath};

use crate::calibration::registration::PhyCalibrationPath;

impl PhyCalibrationPath {
    pub const fn name(self) -> oer_portable_diagnostics::Name<32> {
        match self {
            Self::FullUncached => oer_portable_diagnostics::name!("FullUncached"; 32),
            Self::FullForCache => oer_portable_diagnostics::name!("FullForCache"; 32),
            Self::FullAfterRejectedCache => {
                oer_portable_diagnostics::name!("FullAfterRejectedCache"; 32)
            }
            Self::PartialFromCache => oer_portable_diagnostics::name!("PartialFromCache"; 32),
        }
    }
}

impl DiagnosticPath for PhyCalibrationPath {
    fn push_path(&self, path: &mut DiagPath) {
        path.name(self.name().as_str());
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
    use oer_portable_diagnostics::{DiagPath, DiagnosticPath};

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
