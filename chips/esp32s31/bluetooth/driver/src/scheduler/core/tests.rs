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
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        );
    let clocked = ClockedOwner::for_host_test(partitions.bluetooth);
    let operations = Rc::new(RefCell::new(Vec::new()));
    let hal_operations = Rc::clone(&operations);
    let initialized = {
        let (engine, interrupts) = crate::test_clocks::engine(clocked);
        ControllerHalInitialized::initialize_for_validation(engine, interrupts, |_, _| {
            hal_operations.borrow_mut().push("controller-hal");
        })
    };
    let time_scale = initialized.controller_time_scale();
    let scheduler_operations = Rc::clone(&operations);
    let cells = ControllerEventCells::host();
    let mut scheduler = initialized
        .initialize_scheduler_with(ControllerRuntimeResources::<4, _>::new(&cells), |_| {
            scheduler_operations.borrow_mut().push("scheduler-hardware");
            BluetoothSchedulerHardwareListsCleared::for_host_test()
        })
        .expect("pristine host runtime");
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
    assert_eq!(scheduler.runtime_is_pristine(), Ok(true));
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

fn initialized_for_rejection_test() -> ControllerHalInitialized<crate::test_clocks::ModelMemory> {
    let (_, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        );
    let (engine, interrupts) =
        crate::test_clocks::engine(ClockedOwner::for_host_test(partitions.bluetooth));
    ControllerHalInitialized::initialize_for_validation(engine, interrupts, |_, _| {})
}

#[test]
fn rejected_scheduler_initialization_retains_owners_and_pending_notifications() {
    let cells = ControllerEventCells::host();
    let runtime = ControllerRuntimeResources::<4, _>::new(&cells);
    let (interrupt, _, _) = ControllerRuntimeResources::<4, _>::new(&cells).split();
    interrupt
        .scheduler_wake()
        .publish_class_for_test(crate::interrupt::SchedulerWorkerWakeClass::Ordinary);
    let initialized = initialized_for_rejection_test();
    let scale = initialized.controller_time_scale();
    let mut writes = 0;
    let failure = initialized
        .initialize_scheduler_with(runtime, |_| {
            writes += 1;
            BluetoothSchedulerHardwareListsCleared::for_host_test()
        })
        .err()
        .expect("pending runtime must be refused");
    assert_eq!(
        failure.error,
        super::SchedulerInitializationError::RuntimeNotPristine
    );
    assert_eq!(writes, 0);
    assert_eq!(failure.controller.controller_time_scale(), scale);
    assert_eq!(failure.runtime.is_pristine(), Ok(false));
    assert_eq!(interrupt.scheduler_wake().is_pending(), Ok(true));
    cells
        .clear_notifications_after_reset()
        .expect("host context");
    let _initialized = failure
        .controller
        .initialize_scheduler_with(failure.runtime, |_| {
            writes += 1;
            BluetoothSchedulerHardwareListsCleared::for_host_test()
        })
        .expect("same owners can retry");
    assert_eq!(writes, 1);
}

#[test]
fn foreign_scheduler_context_is_a_checked_rejection_before_hardware_writes() {
    use oer_espressif_interrupt_table::signal::host;
    let cells = ControllerEventCells::new(host::foreign_signal(), host::foreign_signal());
    let mut writes = 0;
    let result = initialized_for_rejection_test().initialize_scheduler_with(
        ControllerRuntimeResources::<4, _>::new(&cells),
        |_| {
            writes += 1;
            BluetoothSchedulerHardwareListsCleared::for_host_test()
        },
    );
    let Err(failure) = result else {
        panic!("foreign context must be refused");
    };
    assert_eq!(
        failure.error,
        super::SchedulerInitializationError::ForeignContext
    );
    assert_eq!(writes, 0);
}
