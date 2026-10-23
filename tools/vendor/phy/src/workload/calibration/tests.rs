use super::*;
fn ready() -> CaptureReady {
    CaptureReady {
        epoch: 1,
        lifecycle: CaptureLifecycle::Cold,
        full_calibration_performed: true,
        path: CalibrationPath::FullForCache,
        projection_schema: 1,
        calibration_words: 20,
        registers: 3,
        analog_bytes: 2,
        channel: 6,
        radio: CaptureRadio::Wifi,
    }
}

#[test]
fn full_capture_contract_rejects_every_other_native_path() {
    for path in [
        CalibrationPath::FullUncached,
        CalibrationPath::FullAfterRejectedCache,
        CalibrationPath::PartialFromCache,
    ] {
        assert!(validate_ready(&CaptureReady { path, ..ready() }, 3, 2).is_err());
    }
}

#[test]
fn incomplete_registration_and_changed_image_lengths_are_rejected() {
    validate_ready(&ready(), 3, 2).unwrap();
    assert!(
        validate_ready(
            &CaptureReady {
                full_calibration_performed: false,
                ..ready()
            },
            3,
            2
        )
        .is_err()
    );
    assert!(
        validate_ready(
            &CaptureReady {
                epoch: 0,
                ..ready()
            },
            3,
            2
        )
        .is_err()
    );
    assert!(validate_ready(&ready(), 4, 2).is_err());
    let window = CaptureWindow {
        epoch: 1,
        first: 2,
        count: 1,
    };
    assert!(check_page(&ready(), window, 2, 2, 3, &[9], 3).is_err());
    assert!(check_page::<u8>(&ready(), window, 1, 2, 3, &[], 3).is_err());
    assert!(check_page(&ready(), window, 1, 2, 4, &[9], 3).is_err());
}
