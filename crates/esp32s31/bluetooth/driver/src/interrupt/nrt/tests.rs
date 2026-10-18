use super::*;

struct Backend {
    acknowledged: Option<BluetoothNrtInterruptAcknowledged>,
    captures: usize,
}

impl NrtInterruptBackend for Backend {
    fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
        self.captures += 1;
        self.acknowledged.take().expect("one NRT epoch")
    }
}

#[test]
fn default_profile_retains_one_epoch_without_synthetic_work() {
    let mut backend = Backend {
        acknowledged: Some(BluetoothNrtInterruptAcknowledged::for_validation()),
        captures: 0,
    };

    let _epoch = execute_nrt_default_interrupt_step(&mut backend);

    assert_eq!(backend.captures, 1);
}
