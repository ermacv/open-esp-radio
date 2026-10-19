use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerInstant;

use super::{ControllerEventCells, ControllerRuntimeResources};

#[test]
fn one_aggregate_starts_as_one_pristine_bounded_epoch() {
    let cells = ControllerEventCells::host();
    let resources = ControllerRuntimeResources::<4>::new(&cells);

    assert_eq!(resources.modem_timer_capacity(), 4);
    assert_eq!(resources.is_pristine(), Ok(true));
}

#[test]
#[should_panic(expected = "at least one modem timer slot")]
fn zero_modem_timer_capacity_profile_is_rejected() {
    let cells = ControllerEventCells::host();
    let _resources = ControllerRuntimeResources::<0>::new(&cells);
}

#[test]
fn split_shares_one_set_of_cells_between_interrupt_and_task_endpoints() {
    let cells = ControllerEventCells::host();
    let (interrupt, mut task, modem_timer) = ControllerRuntimeResources::<4>::new(&cells).split();

    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    assert!(!task.scheduler_finished_lists().is_active());
    assert!(modem_timer.queue_is_empty());
}

#[test]
fn split_assigns_the_timer_queue_only_to_the_modem_task_endpoint() {
    let cells = ControllerEventCells::host();
    let (interrupt, _task, mut modem_timer) = ControllerRuntimeResources::<2>::new(&cells).split();

    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    let token = modem_timer
        .queue
        .schedule(
            BluetoothModemLpTimerInstant::from_bits(10),
            BluetoothModemLpTimerInstant::from_bits(20),
        )
        .expect("the disjoint timer endpoint owns both fixed slots");
    assert!(!modem_timer.queue_is_empty());
    assert!(modem_timer.queue.cancel(token));
    assert!(modem_timer.queue_is_empty());
}

#[test]
fn a_pending_notification_keeps_the_next_epoch_from_starting_until_reset_clears_it() {
    let cells = ControllerEventCells::host();
    let (interrupt, task, _timer) = ControllerRuntimeResources::<2>::new(&cells).split();
    assert_eq!(task.retirement_ready(), Ok(()));
    interrupt
        .scheduler_wake()
        .publish_from_interrupt(crate::interrupt::SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(task.retirement_ready(), Ok(()));
    assert_eq!(
        task.scheduler_wake().is_pending(),
        Ok(true),
        "admission must not discard a publication"
    );
    assert_eq!(
        ControllerRuntimeResources::<2>::new(&cells).is_pristine(),
        Ok(false)
    );
    assert_eq!(cells.clear_notifications_after_reset(), Ok(()));
    assert_eq!(
        ControllerRuntimeResources::<2>::new(&cells).is_pristine(),
        Ok(true)
    );
}

#[test]
fn a_foreign_context_neither_observes_nor_clears_the_cells() {
    use oer_interrupt_table::signal::{ForeignContext, host};

    let cells = ControllerEventCells::new(host::foreign_signal(), host::foreign_signal());
    let (interrupt, _task, _timer) = ControllerRuntimeResources::<2>::new(&cells).split();
    let _ = interrupt
        .scheduler_wake()
        .publish_from_interrupt(crate::interrupt::SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(cells.is_quiet(), Err(ForeignContext));
    assert_eq!(
        ControllerRuntimeResources::<2>::new(&cells).is_pristine(),
        Err(ForeignContext)
    );
    assert_eq!(cells.clear_notifications_after_reset(), Err(ForeignContext));
}
