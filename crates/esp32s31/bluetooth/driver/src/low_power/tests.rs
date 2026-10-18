use oer_esp32s31_hal::{bluetooth::ClockedOwner, root::RadioHardware};

use crate::{
    controller_hal::ControllerHalInitialized,
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
    scheduler::SchedulerInitialized,
};

fn scheduler(cells: &ControllerEventCells) -> SchedulerInitialized<'_, 4> {
    let (_shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    ControllerHalInitialized::initialize_for_validation(
        ClockedOwner::for_validation(partitions.bluetooth),
        |_, _| {},
    )
    .initialize_scheduler_for_validation(ControllerRuntimeResources::new(cells))
}

#[test]
fn scheduler_runtime_split_contains_only_hardware_services() {
    let cells = ControllerEventCells::new();
    let mut scheduler = scheduler(&cells);
    assert!(scheduler.runtime_is_pristine());
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
    let cells = ControllerEventCells::new();
    let (mut controller, timer_hardware) = match scheduler(&cells)
        .try_initialize_low_power_hardware_with(|_| Ok::<_, ()>("timer-hardware"))
    {
        Ok(initialized) => initialized,
        Err(_) => panic!("the injected low-power component must complete"),
    };

    assert_eq!(timer_hardware, "timer-hardware");
    assert!(controller.runtime_is_pristine());
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
    let cells = ControllerEventCells::new();
    let (controller, error) = match scheduler(&cells)
        .try_initialize_low_power_hardware_with(|_| Err::<(), _>("timer-owner-separated"))
    {
        Ok(_) => panic!("the injected lower failure must remain visible"),
        Err(failure) => failure,
    };

    assert_eq!(error, "timer-owner-separated");
    assert!(controller.runtime_is_pristine());
    assert_eq!(controller.modem_timer_capacity(), 4);
}

#[test]
#[should_panic(expected = "the interrupt bank is activated before the runtime splits")]
fn the_runtime_cannot_split_before_the_interrupt_bank_is_activated() {
    let cells = ControllerEventCells::new();
    let _endpoints = scheduler(&cells).split_runtime();
}

#[test]
fn the_task_endpoint_returns_the_actual_task_owner_once_idle() {
    let cells = ControllerEventCells::new();
    let mut scheduler = scheduler(&cells);
    let _interrupts = scheduler.take_interrupt_owner();
    let (_, task, _) = scheduler.split_runtime();
    let Ok(hardware) = task.retire() else {
        panic!("an idle task endpoint retires");
    };
    assert_eq!(
        hardware.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!hardware.controller_time_needs_recheck());
}
