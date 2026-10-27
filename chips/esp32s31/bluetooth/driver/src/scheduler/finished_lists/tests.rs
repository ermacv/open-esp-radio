use crate::interrupt::{SchedulerWakeCell, SchedulerWorkerWakeClass};

use oer_esp32s31_hal::bluetooth::engine::EngineFinishedListObservation;

use super::{
    SchedulerFinishedListBackend, SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};

struct Backend {
    observation: Option<EngineFinishedListObservation>,
}

impl SchedulerFinishedListBackend for Backend {
    fn transfer_scheduler_finished_lists(&mut self) -> EngineFinishedListObservation {
        self.observation.take().expect("one scripted transfer")
    }
}

fn backend(lists: &[u8]) -> Backend {
    Backend {
        observation: Some(
            EngineFinishedListObservation::from_lists_for_host_test(lists)
                .expect("semantic list set is valid"),
        ),
    }
}

fn wake(class: SchedulerWorkerWakeClass) -> crate::interrupt::SchedulerWakeBatch {
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    let _publication = cell.publish_class_for_test(class);
    cell.take()
        .expect("the host executor's context")
        .expect("one scheduler batch must be pending")
        .classify()
        .expect("fault-free capture")
}

fn assert_list(step: SchedulerFinishedListWorkerStep, expected_index: u8, expected_more: bool) {
    match step {
        SchedulerFinishedListWorkerStep::List { observed, more } => {
            assert_eq!(observed.index().get(), expected_index);
            assert_eq!(more, expected_more);
        }
        _ => panic!("the scripted observation must yield one list"),
    }
}

#[test]
fn multiple_lists_are_drained_lowest_first_one_per_step() {
    let mut backend = backend(&[9, 3]);
    let mut worker = SchedulerFinishedListWorker::new();

    worker
        .capture_with(&mut backend, wake(SchedulerWorkerWakeClass::Ordinary))
        .expect("idle worker accepts one transfer");
    assert_eq!(
        worker.capture_with(&mut backend, wake(SchedulerWorkerWakeClass::Marked),),
        Err(SchedulerFinishedListCaptureError::DrainAlreadyActive)
    );
    assert_list(worker.step(), 3, true);
    assert!(worker.is_active());
    assert_list(worker.step(), 9, false);
    assert!(!worker.is_active());
    assert_eq!(worker.step(), SchedulerFinishedListWorkerStep::Idle);
}

#[test]
fn single_list_exhausts_the_capture() {
    let mut backend = backend(&[3]);
    let mut worker = SchedulerFinishedListWorker::new();

    worker
        .capture_with(&mut backend, wake(SchedulerWorkerWakeClass::Ordinary))
        .unwrap();
    assert_list(worker.step(), 3, false);
    assert!(!worker.is_active());
    assert_eq!(worker.step(), SchedulerFinishedListWorkerStep::Idle);
}

#[test]
fn list_zero_precedes_an_unowned_list_without_losing_the_capture() {
    let mut backend = backend(&[0, 3]);
    let mut worker = SchedulerFinishedListWorker::new();

    worker
        .capture_with(&mut backend, wake(SchedulerWorkerWakeClass::Marked))
        .unwrap();
    assert_list(worker.step(), 0, true);
    assert!(worker.is_active());
    assert_list(worker.step(), 3, false);
    assert!(!worker.is_active());
}

#[test]
fn sole_list_zero_exhausts_the_capture() {
    let mut backend = backend(&[0]);
    let mut worker = SchedulerFinishedListWorker::new();

    worker
        .capture_with(&mut backend, wake(SchedulerWorkerWakeClass::Ordinary))
        .unwrap();
    assert_list(worker.step(), 0, false);
    assert!(!worker.is_active());
    assert_eq!(worker.step(), SchedulerFinishedListWorkerStep::Idle);
}

#[test]
fn stopped_capture_borrows_only_the_engine_and_refuses_before_mmio_without_its_receipt() {
    use oer_esp32s31_hal::{bluetooth::ClockedOwner, root::RadioHardware};
    let (_, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        );
    let (mut engine, _interrupts) =
        crate::test_clocks::engine(ClockedOwner::for_host_test(partitions.bluetooth));
    let mut worker = SchedulerFinishedListWorker::new();
    assert_eq!(
        worker.capture_stopped(&mut engine),
        Err(SchedulerFinishedListCaptureError::SchedulerNotStopped)
    );
    assert!(!worker.is_active());
    engine.stopped_for_host_test();
    assert!(engine.scheduler_is_stopped());
}

#[test]
fn an_overflow_only_batch_grants_no_finished_list_transfer() {
    use crate::interrupt::PrimaryInterruptCapture;
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    cell.publish_from_interrupt(PrimaryInterruptCapture::controller_time_overflow_for_test())
        .unwrap();
    let batch = cell.take().unwrap().unwrap().classify().unwrap();
    let mut backend = backend(&[3]);
    let mut worker = SchedulerFinishedListWorker::new();
    worker.capture_with(&mut backend, batch).unwrap();
    assert!(backend.observation.is_some());
    assert_eq!(worker.step(), SchedulerFinishedListWorkerStep::Idle);
}
