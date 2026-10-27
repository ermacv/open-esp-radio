use oer_esp32s31_bluetooth_driver::controller_time::ControllerTimeRangeFault;
use oer_esp32s31_bluetooth_driver::port::Fault;
use oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig;

#[test]
fn hardware_error_reasons_preserve_existing_codes() {
    for (fault, expected) in [
        (Fault::Disabled, 0),
        (Fault::Hardware, 1),
        (Fault::Accounting, 2),
        (Fault::Abandoned, 3),
        (Fault::RejectedTerminal, 4),
        (Fault::ClockBusy, 5),
        (Fault::ClockBegin, 6),
        (Fault::ClockTimeout, 7),
        (Fault::ClockRead, 8),
        (Fault::Deadline, 9),
        (Fault::SchedulerStop, 10),
        (Fault::SchedulerResume, 11),
        (Fault::SchedulerSubmit, 12),
        (Fault::SchedulerWake, 13),
        (Fault::SchedulerComplete, 14),
        (Fault::DtmFinish, 15),
        (Fault::DtmStatus, 16),
        (Fault::TimerInterrupt, 17),
        (Fault::TimerBegin, 18),
        (Fault::TimerStep, 19),
        (Fault::TimerRearm, 20),
        (Fault::StopTimeout, 21),
        (Fault::ResumeTimeout, 22),
        (Fault::SubmitTimeout, 23),
        (Fault::WakeTimeout, 24),
    ] {
        assert_eq!(fault.reason_code(), expected, "{fault:?}");
    }
}

#[test]
fn controller_time_reason_is_const_and_independent_of_observations() {
    const FAULT: Fault = Fault::ControllerTimeRange(ControllerTimeRangeFault {
        raw_ticks: 0,
        overflows: 0,
        scale: BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    });
    const CODE: u8 = FAULT.reason_code();

    assert_eq!(CODE, 25);
    let observed = Fault::ControllerTimeRange(ControllerTimeRangeFault {
        raw_ticks: u32::MAX,
        overflows: u32::MAX,
        scale: BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    });
    assert_eq!(observed.reason_code(), CODE);
}
