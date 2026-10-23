//! List-zero publications through the task HAL owner and the typed P3 replies.

use super::super::interrupt::{SchedulerDoorbell, SchedulerInterruptError, SchedulerTaskHalf};
use crate::bluetooth::engine::ControllerSramLinkAddress;
use crate::bluetooth::{
    BluetoothSchedulerCancellationIndexed, BluetoothSchedulerCancellationRequested,
    BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerExecutionLock,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModify,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
    BluetoothSchedulerLockModifyRequest, BluetoothSchedulerSkipPublished,
    BluetoothSchedulerSkipRequest, ControllerHal,
};

use super::{
    SchedulerHardware, SchedulerHardwareBackend, SchedulerHardwareError, SchedulerStartError,
};
use crate::bluetooth::engine::SchedulerItemSpace;
use crate::bluetooth::engine::scheduler::{
    SchedulerAction, SchedulerIdleInsertion, SchedulerObservation, SchedulerStep,
    SchedulerTransactionFault, SchedulerWait,
};

const LIST: BluetoothSchedulerHardwareListIndex = BluetoothSchedulerHardwareListIndex::ZERO;

use super::HalPublications;

/// The task HAL owner and the typed P3 reply channel for one call.
///
/// Every address it publishes was resolved against the scheduler item space
/// by the powered task runtime: it names a submitted item of a pool whose
/// storage is bound for `'static`.
struct LiveSchedulerBackend<'call, 'registers, S> {
    controller: &'call mut ControllerHal<'registers>,
    storage: &'call S,
    interrupts: &'call mut SchedulerTaskHalf,
}

#[allow(
    unsafe_code,
    reason = "the powered task runtime resolved every address to 'static pool storage and serializes list zero"
)]
impl<S: SchedulerDoorbell> SchedulerHardwareBackend for LiveSchedulerBackend<'_, '_, S> {
    type Publications = HalPublications;
    type StartError = SchedulerInterruptError;

    fn publish_execution_lock(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> BluetoothSchedulerExecutionLock {
        // SAFETY: `at` names a submitted item in 'static pool storage, and
        // the executor keeps it listed until the lock is released.
        unsafe {
            self.controller.admit_scheduler_execution_lock(
                BluetoothSchedulerExecutionLockRequest::new(
                    crate::bluetooth::engine::layout_reference::controller_reference(
                        at.controller_address().address(),
                    ),
                    LIST,
                ),
            )
        }
    }

    fn release_execution_lock(&mut self, lock: BluetoothSchedulerExecutionLock) {
        self.controller.clear_scheduler_execution_lock_start(lock);
    }

    fn publish_execution_modify(
        &mut self,
        list_deletion: bool,
    ) -> BluetoothSchedulerExecutionModify {
        // SAFETY: the executor owns list zero's reconciliation or deletion
        // until it releases execution modify.
        unsafe {
            self.controller
                .admit_scheduler_execution_modify(LIST, list_deletion)
        }
    }

    fn release_execution_modify(&mut self, modify: BluetoothSchedulerExecutionModify) {
        self.controller
            .clear_scheduler_execution_modify_start(modify);
    }

    fn publish_lock_modify(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> BluetoothSchedulerLockModifyPublished {
        // SAFETY: `at` names the newly linked item in 'static pool storage;
        // the executor holds list zero until the request leaves START.
        unsafe {
            self.controller
                .publish_scheduler_lock_modify(BluetoothSchedulerLockModifyRequest::new(
                    crate::bluetooth::engine::layout_reference::controller_reference(
                        at.controller_address().address(),
                    ),
                    LIST,
                ))
        }
    }

    fn publish_head(&mut self, at: Option<ControllerSramLinkAddress>) {
        // SAFETY: the executor finished every descriptor write of the list it
        // publishes; each item lives in 'static pool storage.
        let _published = unsafe {
            self.controller.publish_scheduler_hardware_list_head(
                LIST,
                at.map(|at| {
                    crate::bluetooth::engine::layout_reference::controller_reference(
                        at.controller_address().address(),
                    )
                }),
            )
        };
    }

    fn index_cancellation(&mut self) -> BluetoothSchedulerCancellationIndexed {
        // SAFETY: the executor observed the lock-modify request idle and holds
        // list zero until it releases the cancellation.
        unsafe { self.controller.index_scheduler_cancellation(LIST) }
    }

    async fn acknowledge_cancellation_source(
        &mut self,
        indexed: BluetoothSchedulerCancellationIndexed,
    ) -> Result<
        (
            BluetoothSchedulerCancellationIndexed,
            BluetoothSchedulerCancellationSourceAcknowledged,
        ),
        SchedulerHardwareError,
    > {
        self.interrupts
            .acknowledge_cancellation(self.storage, indexed)
            .await
            .map_err(SchedulerHardwareError::Interrupt)
    }

    fn request_cancellation(
        &mut self,
        indexed: BluetoothSchedulerCancellationIndexed,
        acknowledged: BluetoothSchedulerCancellationSourceAcknowledged,
    ) -> BluetoothSchedulerCancellationRequested {
        self.controller
            .request_scheduler_cancellation(indexed, acknowledged)
    }

    fn release_cancellation(&mut self, cancellation: BluetoothSchedulerCancellationRequested) {
        let _released = self.controller.release_scheduler_cancellation(cancellation);
    }

    fn publish_skip(&mut self, at: ControllerSramLinkAddress) -> BluetoothSchedulerSkipPublished {
        // SAFETY: `at` names a detached item that its pool keeps submitted in
        // 'static storage until the cancellation releases it.
        unsafe {
            self.controller
                .publish_scheduler_skip(BluetoothSchedulerSkipRequest::new(
                    crate::bluetooth::engine::layout_reference::controller_reference(
                        at.controller_address().address(),
                    ),
                    LIST,
                ))
        }
    }

    fn clear_skip(&mut self, skip: BluetoothSchedulerSkipPublished) {
        let _cleared = self.controller.clear_scheduler_skip(skip);
    }

    async fn observe(
        &mut self,
        wait: SchedulerWait,
        lock: Option<&mut BluetoothSchedulerExecutionLock>,
        modify: Option<&mut BluetoothSchedulerExecutionModify>,
        cancellation: Option<&mut BluetoothSchedulerCancellationRequested>,
        skip: Option<&BluetoothSchedulerSkipPublished>,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        if wait == SchedulerWait::ExecutionLock {
            let lock = lock.ok_or(SchedulerHardwareError::NotAwaiting(wait))?;
            return self
                .controller
                .step_scheduler_execution_lock(self.interrupts, lock, self.storage)
                .await
                .map(SchedulerObservation::ExecutionLock)
                .map_err(SchedulerHardwareError::Interrupt);
        }
        if wait == SchedulerWait::ExecutionModify {
            let modify = modify.ok_or(SchedulerHardwareError::NotAwaiting(wait))?;
            return self
                .controller
                .step_scheduler_execution_modify(self.interrupts, modify, self.storage)
                .await
                .map(SchedulerObservation::ExecutionModify)
                .map_err(SchedulerHardwareError::Interrupt);
        }
        if wait == SchedulerWait::LockModify {
            let interrupt = self
                .interrupts
                .lock_modify(self.storage)
                .await
                .map_err(SchedulerHardwareError::Interrupt)?;
            let task = self.controller.capture_scheduler_lock_modify_task();
            return Ok(SchedulerObservation::LockModify(
                BluetoothSchedulerLockModifyObservation::from_split(interrupt, task),
            ));
        }
        let busy = self
            .interrupts
            .busy(self.storage)
            .await
            .map_err(SchedulerHardwareError::Interrupt)?;
        Ok(match (wait, cancellation, skip) {
            (SchedulerWait::Cancellation, Some(requested), _) => {
                SchedulerObservation::Cancellation(
                    self.controller
                        .observe_scheduler_cancellation(requested, busy),
                )
            }
            (SchedulerWait::Skip, _, Some(published)) => {
                SchedulerObservation::Skip(self.controller.observe_scheduler_skip(published, busy))
            }
            _ => return Err(SchedulerHardwareError::NotAwaiting(wait)),
        })
    }

    async fn start(
        &mut self,
        at: ControllerSramLinkAddress,
    ) -> Result<
        BluetoothSchedulerHardwareRunCommandPublished,
        SchedulerStartError<SchedulerInterruptError>,
    > {
        // SAFETY: the executor linked every listed item before returning the
        // head; each lives in 'static pool storage, and the scheduler is idle.
        let published = unsafe {
            self.controller.publish_scheduler_hardware_list_head(
                LIST,
                Some(
                    crate::bluetooth::engine::layout_reference::controller_reference(
                        at.controller_address().address(),
                    ),
                ),
            )
        };
        let interrupts = self
            .interrupts
            .prepare_run(self.storage)
            .await
            .map_err(SchedulerStartError::Interrupts)?;
        let event = self
            .controller
            .publish_scheduler_run_event(published, interrupts);
        Ok(self
            .controller
            .publish_scheduler_hardware_run_command(event))
    }
}

/// The live list-zero operations: each resolves every address a step names
/// to a submitted item of `items`, whose pools bind their storage for
/// `'static`, before any register changes, then performs it through the task
/// HAL owner and the typed P3 reply channel.
impl SchedulerHardware<HalPublications> {
    /// Perform one common stop step through typed P3 acknowledgements.
    /// Resolve a fresh hardware head inside the retained item space.
    pub async fn snapshot_live(
        &mut self,
        controller: &mut ControllerHal<'_>,
        interrupts: &mut SchedulerTaskHalf,
        storage: &impl SchedulerDoorbell,
        items: &SchedulerItemSpace<impl crate::bluetooth::engine::SchedulerItemSources>,
    ) -> Result<
        crate::bluetooth::engine::scheduler::SchedulerHardwareSnapshot,
        SchedulerHardwareError,
    > {
        let busy = interrupts
            .busy(storage)
            .await
            .map_err(SchedulerHardwareError::Interrupt)?;
        let raw = controller.observe_scheduler_hardware_list_head(LIST);
        let head = match raw.address() {
            None => None,
            Some(address) => {
                let link = ControllerSramLinkAddress::new(address.address())
                    .map_err(|_| SchedulerHardwareError::ForeignHead)?;
                Some(
                    items
                        .listed_item(link)
                        .ok_or(SchedulerHardwareError::ForeignHead)?,
                )
            }
        };
        Ok(crate::bluetooth::engine::scheduler::SchedulerHardwareSnapshot { busy, head })
    }

    /// Perform the actions of one executor step on list zero, in order.
    ///
    /// Every named item must be submitted in `items`; otherwise nothing is
    /// performed.
    pub async fn perform_live<I: Copy, const CAPACITY: usize>(
        &mut self,
        controller: &mut ControllerHal<'_>,
        interrupts: &mut SchedulerTaskHalf,
        storage: &impl SchedulerDoorbell,
        items: &SchedulerItemSpace<impl crate::bluetooth::engine::SchedulerItemSources>,
        step: &SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError> {
        for action in step.actions() {
            let link = match action {
                SchedulerAction::PublishExecutionLock(link)
                | SchedulerAction::PublishLockModify(link)
                | SchedulerAction::PublishHead(Some(link))
                | SchedulerAction::PublishSkip(link) => link,
                _ => continue,
            };
            if items.listed_item(link).is_none() {
                return Err(SchedulerHardwareError::ForeignItem);
            }
        }
        let mut backend = LiveSchedulerBackend {
            controller,
            storage,
            interrupts,
        };
        self.perform(&mut backend, step).await
    }

    /// Take one fresh observation of the awaited publication; each
    /// diagnostic sample settles within the named P3 diagnostic budget.
    pub async fn observe_live(
        &mut self,
        controller: &mut ControllerHal<'_>,
        interrupts: &mut SchedulerTaskHalf,
        storage: &impl SchedulerDoorbell,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        let mut backend = LiveSchedulerBackend {
            controller,
            storage,
            interrupts,
        };
        self.observe(&mut backend, wait).await
    }

    /// Clear what a faulted list transaction left published.
    pub fn recover_live(
        &mut self,
        controller: &mut ControllerHal<'_>,
        interrupts: &mut SchedulerTaskHalf,
        storage: &impl SchedulerDoorbell,
        fault: SchedulerTransactionFault,
    ) {
        let mut backend = LiveSchedulerBackend {
            controller,
            storage,
            interrupts,
        };
        self.recover(&mut backend, fault);
    }

    /// Publish the idle insertion's head and start the scheduler with RUN.
    pub async fn start_live<S: SchedulerDoorbell>(
        &mut self,
        controller: &mut ControllerHal<'_>,
        interrupts: &mut SchedulerTaskHalf,
        storage: &S,
        items: &SchedulerItemSpace<impl crate::bluetooth::engine::SchedulerItemSources>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<SchedulerInterruptError>> {
        if items.listed_item(insertion.head).is_none() {
            return Err(SchedulerStartError::ForeignItem);
        }
        let mut backend = LiveSchedulerBackend {
            controller,
            storage,
            interrupts,
        };
        self.start(&mut backend, insertion).await
    }
}
