use std::vec::Vec;

use oer_esp32s31_bluetooth_memory::ControllerSramLinkAddress;
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerCancellationDisposition, BluetoothSchedulerExecutionLockDisposition,
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerSkipDisposition,
    BluetoothSchedulerSkipResult,
};

use super::{
    SchedulerHardware, SchedulerHardwareBackend, SchedulerHardwareError, SchedulerPublications,
    SchedulerStartError,
};
use crate::scheduler::{
    SchedulerAction as Action, SchedulerIdleInsertion, SchedulerObservation,
    SchedulerTransactionFault, SchedulerWait,
};

enum Units {}

impl SchedulerPublications for Units {
    type Lock = ();
    type Modify = ();
    type LockModify = ();
    type Indexed = ();
    type Acknowledged = ();
    type Cancellation = ();
    type Skip = ();
    type Run = ();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Call {
    Action(Action),
    Observe(SchedulerWait),
    Start(ControllerSramLinkAddress),
}

#[derive(Default)]
struct Recorder {
    calls: Vec<Call>,
    interrupts_missing: bool,
    observations: Vec<SchedulerObservation>,
}

impl SchedulerHardwareBackend for Recorder {
    type Publications = Units;
    type StartError = ();

    fn publish_execution_lock(&mut self, at: ControllerSramLinkAddress) {
        self.calls
            .push(Call::Action(Action::PublishExecutionLock(at)));
    }
    fn release_execution_lock(&mut self, (): ()) {
        self.calls.push(Call::Action(Action::ReleaseExecutionLock));
    }
    fn publish_execution_modify(&mut self, list_deletion: bool) {
        self.calls.push(Call::Action(if list_deletion {
            Action::PublishExecutionModifyListDeletion
        } else {
            Action::PublishExecutionModify
        }));
    }
    fn release_execution_modify(&mut self, (): ()) {
        self.calls
            .push(Call::Action(Action::ReleaseExecutionModify));
    }
    fn publish_lock_modify(&mut self, at: ControllerSramLinkAddress) {
        self.calls.push(Call::Action(Action::PublishLockModify(at)));
    }
    fn publish_head(&mut self, head: Option<ControllerSramLinkAddress>) {
        self.calls.push(Call::Action(Action::PublishHead(head)));
    }
    fn index_cancellation(&mut self) {
        self.calls.push(Call::Action(Action::IndexCancellation));
    }
    fn acknowledge_cancellation_source(&mut self, (): &()) -> Result<(), SchedulerHardwareError> {
        if self.interrupts_missing {
            return Err(SchedulerHardwareError::InterruptOwnerUnavailable);
        }
        self.calls
            .push(Call::Action(Action::AcknowledgeCancellationSource));
        Ok(())
    }
    fn request_cancellation(&mut self, (): (), (): ()) {
        self.calls.push(Call::Action(Action::RequestCancellation));
    }
    fn release_cancellation(&mut self, (): ()) {
        self.calls.push(Call::Action(Action::ReleaseCancellation));
    }
    fn publish_skip(&mut self, at: ControllerSramLinkAddress) {
        self.calls.push(Call::Action(Action::PublishSkip(at)));
    }
    fn clear_skip(&mut self, (): ()) {
        self.calls.push(Call::Action(Action::ClearSkip));
    }
    fn observe(
        &mut self,
        wait: SchedulerWait,
        _lock: Option<&mut ()>,
        _modify: Option<&mut ()>,
        _cancellation: Option<&mut ()>,
        _skip: Option<&()>,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        self.calls.push(Call::Observe(wait));
        Ok(self.observations.remove(0))
    }
    fn start(&mut self, head: ControllerSramLinkAddress) -> Result<(), SchedulerStartError<()>> {
        self.calls.push(Call::Start(head));
        Ok(())
    }
}

fn link(offset: u32) -> ControllerSramLinkAddress {
    ControllerSramLinkAddress::new(0x2f00_1000 + offset).unwrap()
}

fn perform(
    hardware: &mut SchedulerHardware<Units>,
    backend: &mut Recorder,
    actions: &[Action],
) -> Result<(), SchedulerHardwareError> {
    hardware.perform_actions(backend, actions.iter().copied())
}

fn lock_modify(start: bool) -> SchedulerObservation {
    SchedulerObservation::LockModify(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, start, 0),
    )
}

#[test]
fn live_insertion_holds_each_publication_until_its_release() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder {
        observations: std::vec![
            SchedulerObservation::ExecutionLock(
                BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained
            ),
            lock_modify(true),
            lock_modify(false),
        ],
        ..Recorder::default()
    };
    perform(
        &mut hardware,
        &mut backend,
        &[Action::PublishExecutionLock(link(0))],
    )
    .unwrap();
    hardware
        .observe(&mut backend, SchedulerWait::ExecutionLock)
        .unwrap();
    perform(
        &mut hardware,
        &mut backend,
        &[Action::PublishLockModify(link(0x80))],
    )
    .unwrap();
    // The lock stays until the lock-modify request leaves START.
    assert_eq!(
        perform(&mut hardware, &mut backend, &[Action::ReleaseExecutionLock]),
        Err(SchedulerHardwareError::OutOfOrder(
            Action::ReleaseExecutionLock
        ))
    );
    hardware
        .observe(&mut backend, SchedulerWait::LockModify)
        .unwrap();
    hardware
        .observe(&mut backend, SchedulerWait::LockModify)
        .unwrap();
    perform(&mut hardware, &mut backend, &[Action::ReleaseExecutionLock]).unwrap();
    assert!(hardware.is_quiet());
    assert_eq!(
        backend.calls,
        [
            Call::Action(Action::PublishExecutionLock(link(0))),
            Call::Observe(SchedulerWait::ExecutionLock),
            Call::Action(Action::PublishLockModify(link(0x80))),
            Call::Observe(SchedulerWait::LockModify),
            Call::Observe(SchedulerWait::LockModify),
            Call::Action(Action::ReleaseExecutionLock),
        ]
    );
}

#[test]
fn a_step_out_of_order_changes_no_register() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder::default();
    assert_eq!(
        perform(
            &mut hardware,
            &mut backend,
            &[Action::IndexCancellation, Action::PublishSkip(link(0))],
        ),
        Err(SchedulerHardwareError::OutOfOrder(Action::PublishSkip(
            link(0)
        )))
    );
    assert_eq!(
        hardware.observe(&mut backend, SchedulerWait::ExecutionModify),
        Err(SchedulerHardwareError::NotAwaiting(
            SchedulerWait::ExecutionModify
        ))
    );
    assert!(backend.calls.is_empty());
    assert!(hardware.is_quiet());
}

#[test]
fn cancellation_opens_the_hold_skips_and_closes_it() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder {
        observations: std::vec![
            lock_modify(false),
            SchedulerObservation::Cancellation(BluetoothSchedulerCancellationDisposition::Settled),
            SchedulerObservation::Skip(BluetoothSchedulerSkipDisposition::Completed(
                BluetoothSchedulerSkipResult::One
            )),
        ],
        ..Recorder::default()
    };
    // Cancellation first waits for any earlier lock-modify request.
    hardware
        .observe(&mut backend, SchedulerWait::LockModify)
        .unwrap();
    perform(
        &mut hardware,
        &mut backend,
        &[
            Action::IndexCancellation,
            Action::AcknowledgeCancellationSource,
            Action::RequestCancellation,
        ],
    )
    .unwrap();
    hardware
        .observe(&mut backend, SchedulerWait::Cancellation)
        .unwrap();
    perform(
        &mut hardware,
        &mut backend,
        &[Action::PublishSkip(link(0x40))],
    )
    .unwrap();
    hardware.observe(&mut backend, SchedulerWait::Skip).unwrap();
    assert_eq!(
        perform(&mut hardware, &mut backend, &[Action::ReleaseCancellation]),
        Err(SchedulerHardwareError::OutOfOrder(
            Action::ReleaseCancellation
        ))
    );
    perform(
        &mut hardware,
        &mut backend,
        &[Action::ClearSkip, Action::ReleaseCancellation],
    )
    .unwrap();
    assert!(hardware.is_quiet());
    assert_eq!(backend.calls.len(), 9);
}

#[test]
fn a_missing_interrupt_owner_stops_the_step_at_the_acknowledgement() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder {
        interrupts_missing: true,
        ..Recorder::default()
    };
    assert_eq!(
        perform(
            &mut hardware,
            &mut backend,
            &[
                Action::IndexCancellation,
                Action::AcknowledgeCancellationSource,
                Action::RequestCancellation,
            ],
        ),
        Err(SchedulerHardwareError::InterruptOwnerUnavailable)
    );
    assert_eq!(backend.calls, [Call::Action(Action::IndexCancellation)]);
    assert!(!hardware.is_quiet());
}

#[test]
fn recovery_clears_what_the_faulted_transaction_published() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder::default();
    perform(
        &mut hardware,
        &mut backend,
        &[
            Action::IndexCancellation,
            Action::AcknowledgeCancellationSource,
            Action::RequestCancellation,
            Action::PublishSkip(link(0)),
        ],
    )
    .unwrap();
    backend.calls.clear();
    hardware.recover(
        &mut backend,
        SchedulerTransactionFault::UnsupportedSkipResult,
    );
    assert_eq!(
        backend.calls,
        [
            Call::Action(Action::ClearSkip),
            Call::Action(Action::ReleaseCancellation)
        ]
    );
    assert!(hardware.is_quiet());
}

#[test]
fn the_scheduler_starts_only_without_a_transaction() {
    let mut hardware = SchedulerHardware::<Units>::new();
    let mut backend = Recorder::default();
    perform(
        &mut hardware,
        &mut backend,
        &[Action::PublishExecutionModify],
    )
    .unwrap();
    assert_eq!(
        hardware.start(&mut backend, SchedulerIdleInsertion { head: link(0) }),
        Err(SchedulerStartError::TransactionActive)
    );
    perform(
        &mut hardware,
        &mut backend,
        &[Action::ReleaseExecutionModify],
    )
    .unwrap();
    assert!(!hardware.has_run());
    hardware
        .start(&mut backend, SchedulerIdleInsertion { head: link(0) })
        .unwrap();
    assert!(hardware.has_run());
    assert_eq!(backend.calls.last(), Some(&Call::Start(link(0))));
}
