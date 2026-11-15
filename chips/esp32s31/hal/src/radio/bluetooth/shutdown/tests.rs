use super::*;
/// Whether a controller time event is pending.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Time {
    Clear,
    Pending,
}
/// Whether the low-power timer's owner is separated from the controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Timer {
    Present,
    Separated,
}
/// Whether the baseband is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Btbb {
    Free,
    Held,
}
/// What the controller reset reads back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reset {
    Held,
    Released,
}
struct Model {
    pending: Time,
    separated: Timer,
    btbb: Btbb,
    reset_ok: Reset,
    counter_running: bool,
    reset_count: usize,
}
impl Control for Model {
    fn time_pending(&self) -> bool {
        self.pending == Time::Pending
    }
    fn timer_separated(&self) -> bool {
        self.separated == Timer::Separated
    }
    fn btbb_held(&self) -> bool {
        self.btbb == Btbb::Held
    }
    fn reset_controller(&mut self) {
        self.reset_count += 1;
        // SOURCE: HW6_S31_LP_RESET_RETENTION_20261026,
        // hw6-20261026-reset-02: Controller reset leaves LP running.
    }
    fn reset_released(&self) -> bool {
        self.reset_ok == Reset::Released
    }
}
#[test]
fn outstanding_time_timer_or_baseband_prevent_shutdown_mutation() {
    for (pending, separated, btbb, error) in [
        (
            Time::Pending,
            Timer::Separated,
            Btbb::Free,
            BluetoothShutdownError::ControllerTimePending,
        ),
        (
            Time::Clear,
            Timer::Present,
            Btbb::Free,
            BluetoothShutdownError::TimerOwnerPresent,
        ),
        (
            Time::Clear,
            Timer::Separated,
            Btbb::Held,
            BluetoothShutdownError::BtbbHeld,
        ),
    ] {
        let mut model = Model {
            pending,
            separated,
            btbb,
            reset_ok: Reset::Released,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(prepare(&mut model), Err(error));
        assert!(model.counter_running);
        assert_eq!(model.reset_count, 0);
    }
}
#[test]
fn reset_readback_admits_controller_reunion_but_does_not_stop_lp() {
    for reset_ok in [Reset::Held, Reset::Released] {
        let mut model = Model {
            pending: Time::Clear,
            separated: Timer::Separated,
            btbb: Btbb::Free,
            reset_ok,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(
            prepare(&mut model),
            if reset_ok == Reset::Released {
                Ok(())
            } else {
                Err(BluetoothShutdownError::ControllerReset)
            }
        );
        assert_eq!(model.reset_count, 1);
        assert!(model.counter_running, "Controller reset does not stop LP");
    }
}
