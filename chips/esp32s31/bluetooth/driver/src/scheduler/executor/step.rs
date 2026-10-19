//! Hardware actions, waits and observations shared by the list transactions.

use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, SchedulerItemCompletionStatus};
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerCancellationDisposition, BluetoothSchedulerExecutionLockDisposition,
    BluetoothSchedulerExecutionModifyDisposition, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerSkipDisposition,
};

/// One hardware action of a list transaction, performed in order on list
/// zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerAction {
    /// Publish the execution lock at this listed item.
    PublishExecutionLock(ControllerSramLinkAddress),
    /// Clear the execution-lock START.
    ReleaseExecutionLock,
    /// Publish execution modify for the list in insertion mode.
    PublishExecutionModify,
    /// Publish execution modify for the list in list-deletion mode.
    PublishExecutionModifyListDeletion,
    /// Clear the execution-modify START.
    ReleaseExecutionModify,
    /// Publish a lock-modify request for this item.
    PublishLockModify(ControllerSramLinkAddress),
    /// Publish this item as the list head, or clear the head.
    PublishHead(Option<ControllerSramLinkAddress>),
    /// Publish the list index of the cancellation hold.
    IndexCancellation,
    /// Acknowledge the cancellation interrupt source through the interrupt
    /// owner.
    AcknowledgeCancellationSource,
    /// Open the cancellation hold.
    RequestCancellation,
    /// Publish a skip request for this detached item.
    PublishSkip(ControllerSramLinkAddress),
    /// Clear the skip request after its terminal observation.
    ClearSkip,
    /// Close the cancellation hold.
    ReleaseCancellation,
}

/// Hardware observation that a transaction waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWait {
    ExecutionLock,
    ExecutionModify,
    LockModify,
    Cancellation,
    Skip,
}

/// Observation delivered to a waiting transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerObservation {
    ExecutionLock(BluetoothSchedulerExecutionLockDisposition),
    ExecutionModify(BluetoothSchedulerExecutionModifyDisposition),
    LockModify(BluetoothSchedulerLockModifyObservation),
    Cancellation(BluetoothSchedulerCancellationDisposition),
    Skip(BluetoothSchedulerSkipDisposition),
}

/// What follows the actions of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerNext {
    /// Deliver the next observation of this kind.
    Await(SchedulerWait),
    /// The transaction is complete; the mirror accepts other operations
    /// again.
    Finished,
}

/// Items that left the list and returned to their owner, with the status
/// sampled when the transaction finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerReleased<I, const CAPACITY: usize> {
    items: [Option<(I, Option<SchedulerItemCompletionStatus>)>; CAPACITY],
    len: usize,
}

impl<I: Copy, const CAPACITY: usize> SchedulerReleased<I, CAPACITY> {
    pub(super) const fn new() -> Self {
        Self {
            items: [None; CAPACITY],
            len: 0,
        }
    }

    pub(super) fn push(&mut self, id: I, status: Option<SchedulerItemCompletionStatus>) {
        self.items[self.len] = Some((id, status));
        self.len += 1;
    }

    /// Released items in list order. `None` means hardware had not executed
    /// the item.
    pub fn iter(&self) -> impl Iterator<Item = (I, Option<SchedulerItemCompletionStatus>)> + '_ {
        self.items[..self.len].iter().flatten().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Actions to perform in order, then the next expectation. A finished
/// cancellation or flush also releases its items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the actions must be performed before the next observation"]
pub struct SchedulerStep<I, const CAPACITY: usize> {
    actions: [Option<SchedulerAction>; 3],
    next: SchedulerNext,
    released: SchedulerReleased<I, CAPACITY>,
}

impl<I: Copy, const CAPACITY: usize> SchedulerStep<I, CAPACITY> {
    pub(super) fn new(actions: &[SchedulerAction], next: SchedulerNext) -> Self {
        let mut slots = [None; 3];
        for (slot, action) in slots.iter_mut().zip(actions) {
            *slot = Some(*action);
        }
        Self {
            actions: slots,
            next,
            released: SchedulerReleased::new(),
        }
    }

    pub(super) fn wait(wait: SchedulerWait) -> Self {
        Self::new(&[], SchedulerNext::Await(wait))
    }

    pub(super) fn finished(
        actions: &[SchedulerAction],
        released: SchedulerReleased<I, CAPACITY>,
    ) -> Self {
        Self {
            released,
            ..Self::new(actions, SchedulerNext::Finished)
        }
    }

    /// Actions to perform, in order.
    pub fn actions(&self) -> impl Iterator<Item = SchedulerAction> + Clone + '_ {
        self.actions.iter().flatten().copied()
    }

    pub const fn next(&self) -> SchedulerNext {
        self.next
    }

    /// Items the finished transaction returned to their owner. They may be
    /// reused only after the actions of this step are performed.
    pub const fn released(&self) -> &SchedulerReleased<I, CAPACITY> {
        &self.released
    }
}

/// Why a transaction ended early, or why an observation was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTransactionFault {
    /// No transaction is in progress.
    NoTransaction,
    /// The observation does not match the awaited kind; the transaction
    /// continues to wait.
    UnexpectedObservation,
    /// The execution lock returned a result on which the vendor asserts. The
    /// insertion ends without linking its event; the caller must still clear
    /// the execution-lock START.
    UnsupportedExecutionLockResult,
    /// Execution modify reported the status the vendor treats as impossible.
    /// The list is unchanged; the caller must still clear the
    /// execution-modify START.
    ExecutionModifyRejected,
    /// The skip request returned the result on which the vendor asserts. The
    /// caller must still clear the skip request and close the hold. The
    /// detached items stay withheld: hardware may still reach them, so only
    /// a Bluetooth core reset returns their memory.
    UnsupportedSkipResult,
}
