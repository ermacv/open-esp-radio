use super::{BluetoothBasebandV2TransitionPort, execute_standalone_bluetooth_transition};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoundaryStep {
    Body(u8),
    DeviceFence,
}

#[derive(Default)]
struct BoundaryTrace {
    steps: Vec<BoundaryStep>,
}

impl BluetoothBasebandV2TransitionPort for BoundaryTrace {
    fn execute_body(&mut self, gain_parameter: u8) {
        self.steps.push(BoundaryStep::Body(gain_parameter));
    }

    fn order_device_accesses(&mut self) {
        self.steps.push(BoundaryStep::DeviceFence);
    }
}

#[test]
fn shared_transition_closes_the_body_with_one_fence() {
    let mut bluetooth = BoundaryTrace::default();
    execute_standalone_bluetooth_transition(&mut bluetooth, 0x6d);
    assert_eq!(
        bluetooth.steps,
        [BoundaryStep::Body(0x6d), BoundaryStep::DeviceFence]
    );
}
