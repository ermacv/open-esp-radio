use super::*;
use crate::tracking::calibration::PhyCalibrationTrackingRequest;
use crate::tracking::calibration::{
    PhyCalibrationTrackingAction, PhyCalibrationTrackingTransition,
};
use crate::tracking::parameters::PhyCalibrationTrackClass;

const PARAMETERS: PhyCalibrationTrackingParameters = PhyCalibrationTrackingParameters {
    current_temperature: 50,
    common_reference_temperature: 21,
    transmit_reference_temperature: 20,

    threshold_override: None,
    current_channel: 13,
    channel_bandwidth: 1,
    crystal_selector: 0x31,
};

#[test]
fn independent_references_select_only_due_hardware_branches() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        let request = PhyCalibrationTrackingRequest {
            clients: class.clients(),
        };
        let decision = PARAMETERS.decision();
        assert!(!decision.common.is_due());
        assert!(decision.transmit.is_due());
        assert_eq!(decision.transmit.delta(), 30);
        assert_eq!(
            PhyCalibrationTrackingTransition::new(request, PARAMETERS).action(),
            PhyCalibrationTrackingAction::SetGrantProtect { enabled: true },
        );
        let unchanged = PhyCalibrationTrackingParameters {
            transmit_reference_temperature: 50,

            ..PARAMETERS
        };
        assert!(matches!(
            PhyCalibrationTrackingTransition::new(request, unchanged).action(),
            PhyCalibrationTrackingAction::Complete(outcome)
                if !outcome.common_updated && !outcome.transmit_updated
        ));
        let forced = PhyCalibrationTrackingParameters {
            threshold_override: Some(0),
            ..unchanged
        };
        assert!(forced.decision().transmit.is_due());
        assert_eq!(
            PhyCalibrationTrackingTransition::new(request, forced).action(),
            PhyCalibrationTrackingAction::SetGrantProtect { enabled: true },
        );
        // Observing demand did not commit or overwrite the source references.
        assert_eq!(unchanged.transmit_reference_temperature, 50);
        assert_eq!(unchanged.common_reference_temperature, 21);
    }
}

#[test]
fn signed_temperature_extremes_do_not_wrap_and_threshold_is_inclusive() {
    for (current, reference, delta, due) in [
        (i16::MIN, i16::MAX, 65_535, true),
        (i16::MAX, i16::MIN, 65_535, true),
        (-10, 20, 30, true),
        (20, -10, 30, true),
        (20, -9, 29, false),
    ] {
        let demand = ThermalDemand {
            current,
            reference,
            threshold: 30,
        };
        assert_eq!(demand.delta(), delta);
        assert_eq!(demand.is_due(), due);
    }
}
