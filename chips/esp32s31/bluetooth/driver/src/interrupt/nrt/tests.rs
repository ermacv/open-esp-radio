use super::*;

struct Backend {
    acknowledged: Option<BluetoothNrtInterruptAcknowledged>,
    captures: usize,
}

impl NrtInterruptBackend for Backend {
    fn capture_nrt_and_acknowledge(&mut self) -> Option<BluetoothNrtInterruptAcknowledged> {
        self.captures += 1;
        self.acknowledged.take()
    }
}

#[test]
fn default_profile_retains_one_epoch_without_synthetic_work() {
    let mut backend = Backend {
        acknowledged: Some(BluetoothNrtInterruptAcknowledged::for_host_test()),
        captures: 0,
    };

    assert!(execute_nrt_default_interrupt_step(&mut backend).is_some());
    assert_eq!(backend.captures, 1);
}

#[test]
fn clear_snapshots_make_no_epoch() {
    let mut backend = Backend {
        acknowledged: None,
        captures: 0,
    };

    assert!(execute_nrt_default_interrupt_step(&mut backend).is_none());
    assert_eq!(backend.captures, 1);
}
