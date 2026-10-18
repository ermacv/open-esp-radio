use crate::{
    controller_hal::ControllerHalInitialized,
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
};

use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_hal::{
    bluetooth::{BluetoothSchedulerHardwareListsCleared, ClockedOwner},
    root::RadioHardware,
};

#[test]
fn controller_hal_precedes_complete_scheduler_init() {
    let (_shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    let clocked = ClockedOwner::for_validation(partitions.bluetooth);
    let operations = Rc::new(RefCell::new(Vec::new()));
    let hal_operations = Rc::clone(&operations);
    let initialized = ControllerHalInitialized::initialize_for_validation(clocked, |_, _| {
        hal_operations.borrow_mut().push("controller-hal");
    });
    let time_scale = initialized.controller_time_scale();
    let scheduler_operations = Rc::clone(&operations);
    let cells = ControllerEventCells::new();
    let mut scheduler =
        initialized.initialize_scheduler_with(ControllerRuntimeResources::<4>::new(&cells), |_| {
            scheduler_operations.borrow_mut().push("scheduler-hardware");
            BluetoothSchedulerHardwareListsCleared::for_validation()
        });
    assert_eq!(
        operations.borrow().as_slice(),
        ["controller-hal", "scheduler-hardware"]
    );
    assert_eq!(scheduler.controller_time_scale(), time_scale);
    assert_eq!(
        scheduler.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!scheduler.controller_time_needs_recheck());
    assert_eq!(scheduler.modem_timer_capacity(), 4);
    assert!(scheduler.runtime_is_pristine());
    let _interrupts = scheduler.take_interrupt_owner();
    let (interrupt, task, _modem_timer) = scheduler.split_runtime();
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert_eq!(
        task.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!task.controller_time_needs_recheck());
}
