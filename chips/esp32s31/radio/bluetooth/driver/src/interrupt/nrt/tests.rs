use super::*;

struct Backend {
    acknowledged: Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault>,
    captures: usize,
}

impl NrtInterruptBackend for Backend {
    fn capture_nrt_and_acknowledge(
        &mut self,
    ) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault> {
        self.captures += 1;
        core::mem::replace(&mut self.acknowledged, Ok(None))
    }
}

#[test]
fn default_profile_retains_one_epoch_without_synthetic_work() {
    let mut backend = Backend {
        acknowledged: Ok(Some(BluetoothNrtInterruptAcknowledged::for_host_test())),
        captures: 0,
    };

    assert!(
        execute_nrt_default_interrupt_step(&mut backend)
            .unwrap()
            .is_some()
    );
    assert_eq!(backend.captures, 1);
}

#[test]
fn clear_snapshots_make_no_epoch() {
    let mut backend = Backend {
        acknowledged: Ok(None),
        captures: 0,
    };

    assert!(
        execute_nrt_default_interrupt_step(&mut backend)
            .unwrap()
            .is_none()
    );
    assert_eq!(backend.captures, 1);
}

#[test]
fn unexpected_primary_clear_lane_retains_both_nrt_observations_as_a_fault() {
    let fault = BluetoothNrtInterruptFault {
        snapshots: [0x98765432, 0xabcdef02],
    };
    let mut backend = Backend {
        acknowledged: Err(fault),
        captures: 0,
    };
    assert_eq!(execute_nrt_default_interrupt_step(&mut backend), Err(fault));
    assert_eq!(backend.captures, 1);
}
