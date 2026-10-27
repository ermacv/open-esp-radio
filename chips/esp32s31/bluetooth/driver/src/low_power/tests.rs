use oer_esp32s31_hal::{bluetooth::ClockedOwner, root::RadioHardware};

use crate::{
    controller_hal::ControllerHalInitialized,
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
    scheduler::SchedulerInitialized,
};

fn scheduler(
    cells: &ControllerEventCells<oer_espressif_interrupt_table::signal::host::Deferred>,
) -> SchedulerInitialized<
    '_,
    4,
    oer_espressif_interrupt_table::signal::host::Deferred,
    crate::test_clocks::ModelMemory,
> {
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        );
    let (engine, interrupts) =
        crate::test_clocks::engine(ClockedOwner::for_host_test(partitions.bluetooth));
    ControllerHalInitialized::initialize_for_validation(engine, interrupts, |_, _| {})
        .initialize_scheduler_for_validation(ControllerRuntimeResources::new(cells))
        .expect("pristine host runtime")
}

#[test]
fn scheduler_runtime_split_contains_only_hardware_services() {
    let cells = ControllerEventCells::host();
    let mut scheduler = scheduler(&cells);
    assert_eq!(scheduler.runtime_is_pristine(), Ok(true));
    let _interrupts = scheduler.take_interrupt_owner();
    let (interrupt, task, modem_timer) = scheduler.split_runtime();
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    assert!(modem_timer.queue_is_empty());
}

#[test]
fn low_power_hardware_stays_in_the_same_pristine_controller_epoch() {
    let cells = ControllerEventCells::host();
    let (mut controller, timer_hardware) = match scheduler(&cells)
        .try_initialize_low_power_hardware_with(|_| Ok::<_, ()>("timer-hardware"))
    {
        Ok(initialized) => initialized,
        Err(_) => panic!("the injected low-power component must complete"),
    };

    assert_eq!(timer_hardware, "timer-hardware");
    assert_eq!(controller.runtime_is_pristine(), Ok(true));
    assert_eq!(controller.modem_timer_capacity(), 4);
    let _interrupts = controller.take_interrupt_owner();
    let (interrupt, task, _) = controller.split_runtime();
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
}

#[test]
fn low_power_hardware_failure_returns_the_complete_scheduler_epoch() {
    let cells = ControllerEventCells::host();
    let (controller, error) = match scheduler(&cells)
        .try_initialize_low_power_hardware_with(|_| Err::<(), _>("timer-owner-separated"))
    {
        Ok(_) => panic!("the injected lower failure must remain visible"),
        Err(failure) => failure,
    };

    assert_eq!(error, "timer-owner-separated");
    assert_eq!(controller.runtime_is_pristine(), Ok(true));
    assert_eq!(controller.modem_timer_capacity(), 4);
}

#[test]
#[should_panic(expected = "the interrupt bank is activated before the runtime splits")]
fn the_runtime_cannot_split_before_the_interrupt_bank_is_activated() {
    let cells = ControllerEventCells::host();
    let _endpoints = scheduler(&cells).split_runtime();
}

#[test]
fn the_task_endpoint_returns_the_actual_task_owner_once_idle() {
    let cells = ControllerEventCells::host();
    let mut scheduler = scheduler(&cells);
    let _interrupts = scheduler.take_interrupt_owner();
    let (_, mut task, _) = scheduler.split_runtime();
    let Err((error, mut task)) = task.retire() else {
        panic!("stop receipt required");
    };
    assert_eq!(
        error,
        crate::runtime_resources::ControllerTaskRetirementError::EngineNotStopped
    );
    task.task_owner().engine().stopped_for_host_test();
    let Ok(hardware) = task.retire() else {
        panic!("an idle task endpoint retires");
    };
    assert_eq!(
        hardware.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!hardware.controller_time_needs_recheck());
}
