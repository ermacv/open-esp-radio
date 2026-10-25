use super::*;
struct Model {
    pending: bool,
    separated: bool,
    btbb: bool,
    reset_ok: bool,
    counter_running: bool,
    reset_count: usize,
}
impl Control for Model {
    fn time_pending(&self) -> bool {
        self.pending
    }
    fn timer_separated(&self) -> bool {
        self.separated
    }
    fn btbb_held(&self) -> bool {
        self.btbb
    }
    fn reset_controller(&mut self) {
        self.reset_count += 1;
        if self.reset_ok {
            self.counter_running = false;
        }
    }
    fn reset_released(&self) -> bool {
        self.reset_ok
    }
}
#[test]
fn outstanding_time_timer_or_baseband_prevent_shutdown_mutation() {
    for (pending, separated, btbb, error) in [
        (
            true,
            true,
            false,
            BluetoothShutdownError::ControllerTimePending,
        ),
        (
            false,
            false,
            false,
            BluetoothShutdownError::TimerOwnerPresent,
        ),
        (false, true, true, BluetoothShutdownError::BtbbHeld),
    ] {
        let mut model = Model {
            pending,
            separated,
            btbb,
            reset_ok: true,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(prepare(&mut model), Err(error));
        assert!(model.counter_running);
        assert_eq!(model.reset_count, 0);
    }
}
#[test]
fn reset_readback_is_required_before_returning_hardware_ownership() {
    for reset_ok in [false, true] {
        let mut model = Model {
            pending: false,
            separated: true,
            btbb: false,
            reset_ok,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(
            prepare(&mut model),
            if reset_ok {
                Ok(())
            } else {
                Err(BluetoothShutdownError::ControllerReset)
            }
        );
        assert_eq!(model.reset_count, 1);
        assert_eq!(model.counter_running, !reset_ok);
    }
}
