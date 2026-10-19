use std::vec::Vec;

use super::*;

#[derive(Debug, Eq, PartialEq)]
enum Operation {
    Initialization,
    Calibration,
}

#[derive(Default)]
struct FakePlatform {
    operations: Vec<Operation>,
}

impl PhyPowerDetectorPlatformControl for FakePlatform {
    fn select_power_detector_initialization_mode(&mut self) {
        self.operations.push(Operation::Initialization);
    }

    fn select_power_detector_calibration_mode(&mut self) {
        self.operations.push(Operation::Calibration);
    }
}

#[test]
fn public_api_exposes_only_rom_evidenced_encodings() {
    let mut platform = FakePlatform::default();
    select_initialization_mode(&mut platform);
    select_enabled_mode(&mut platform);
    select_calibration_mode(&mut platform);
    assert_eq!(
        platform.operations,
        [
            Operation::Initialization,
            Operation::Initialization,
            Operation::Calibration,
        ]
    );
}
