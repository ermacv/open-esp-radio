//! Scheduler event executor core for one hardware list.
//!
//! The executor owns the ordered mirror of the hardware list and keeps the
//! scheduler links of every listed item consistent with it. It changes item
//! memory only through [`SchedulerItemAccess`] and never touches a register:
//! each operation returns the hardware action its caller must perform with
//! the HAL. Conflicts are resolved before submission; an overlapping window is
//! rejected, not moved.
//!
//! The executor implements insertion while the scheduler is idle, insertion
//! into a running list through the vendor lock and modify transactions,
//! cancellation of listed events, deletion of the whole list, the completion
//! walk and the stopped state. At most one list transaction runs at a time;
//! while it runs the mirror is frozen and every other operation is refused.

#![forbid(unsafe_code)]
#![allow(
    private_bounds,
    reason = "only HAL-owned item spaces implement physical item access"
)]

use crate::bluetooth::engine::{
    ControllerSramLinkAddress, SchedulerItemCompletionStatus, SchedulerItemId, SchedulerItemSpace,
};
use crate::bluetooth::{BluetoothSchedulerBusyObservation, BluetoothSchedulerStopped};

use crate::bluetooth::engine::scheduler::{
    list::{SchedulerList, SchedulerListInsertError},
    window::SchedulerRawWindow,
};

/// Scheduler-owned fields of the items named by event identities.
///
/// Implementations write the item memory of the event slot pools. Every
/// method addresses an item that the executor currently owns.
pub(crate) trait SchedulerItemAccess<I> {
    /// Controller link of the item.
    fn link(&self, id: I) -> ControllerSramLinkAddress;

    /// Prepare an item for insertion as list insertion does: clear the
    /// scheduler control byte, mark it unexecuted and install both links.
    fn prepare_for_list(
        &mut self,
        id: I,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    );

    /// Replace the hardware next link of a listed item.
    fn set_next(&mut self, id: I, next: Option<ControllerSramLinkAddress>);

    /// Replace the previous-item link of a listed item.
    fn set_previous(&mut self, id: I, previous: Option<ControllerSramLinkAddress>);

    /// Recorded status, or `None` while hardware has not executed the item.
    fn completion_status(&self, id: I) -> Option<SchedulerItemCompletionStatus>;

    /// Mark an item as deleted before it leaves the list, as vendor
    /// cancellation does: set the skip marker in its hardware link word and
    /// the deleted flag. Its own hardware next link is kept, so hardware that
    /// already holds the item can still follow the chain.
    fn mark_deleted(&mut self, id: I);
}

/// The item memory of the role instance pools.
impl SchedulerItemAccess<SchedulerItemId> for SchedulerItemSpace<'_> {
    fn link(&self, id: SchedulerItemId) -> ControllerSramLinkAddress {
        SchedulerItemSpace::link(self, id)
    }

    fn prepare_for_list(
        &mut self,
        id: SchedulerItemId,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        SchedulerItemSpace::prepare_for_list(self, id, previous, next);
    }

    fn set_next(&mut self, id: SchedulerItemId, next: Option<ControllerSramLinkAddress>) {
        SchedulerItemSpace::set_next(self, id, next);
    }

    fn set_previous(&mut self, id: SchedulerItemId, previous: Option<ControllerSramLinkAddress>) {
        SchedulerItemSpace::set_previous(self, id, previous);
    }

    fn completion_status(&self, id: SchedulerItemId) -> Option<SchedulerItemCompletionStatus> {
        SchedulerItemSpace::completion_status(self, id)
    }

    fn mark_deleted(&mut self, id: SchedulerItemId) {
        SchedulerItemSpace::mark_deleted(self, id);
    }
}

/// Why an event was not submitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerSubmitError<I> {
    /// The mirror rejected the window.
    List(SchedulerListInsertError<I>),
    /// The scheduler is running; insertion needs a live-list transaction.
    SchedulerBusy,
    /// The scheduler is idle; insertion does not need a live-list transaction.
    SchedulerIdle,
    /// A list transaction is in progress; the mirror is frozen until it ends.
    TransactionActive,
    /// The executor holds the stopped receipt; nothing may start the
    /// scheduler before it resumes.
    Stopped,
}

/// The mirror is frozen by a list transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerTransactionActive;

/// Why the executor refused a stopped receipt. The receipt is returned.
#[derive(Debug)]
pub enum SchedulerStopRejected {
    /// A list transaction is in progress and must finish first.
    TransactionActive(BluetoothSchedulerStopped),
    /// The executor already holds a stopped receipt.
    AlreadyStopped(BluetoothSchedulerStopped),
}

/// The executor does not hold a stopped receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerNotStopped;

/// Hardware action after an insertion into an idle scheduler.
///
/// The caller publishes `head` as the hardware-list head and then starts the
/// scheduler with the prepared run interrupts and RUN command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the list head must be published and the scheduler started"]
pub struct SchedulerIdleInsertion {
    pub(crate) head: ControllerSramLinkAddress,
}

/// Events that hardware executed, taken from the head of the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerCompletion<I, const CAPACITY: usize> {
    completed: [Option<(I, SchedulerItemCompletionStatus)>; CAPACITY],
    len: usize,
    out_of_order: bool,
}

impl<I: Copy, const CAPACITY: usize> SchedulerCompletion<I, CAPACITY> {
    /// Completed events in execution order with their recorded status.
    pub fn iter(&self) -> impl Iterator<Item = (I, SchedulerItemCompletionStatus)> + '_ {
        self.completed[..self.len].iter().flatten().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether an executed event follows an unexecuted one. The executor
    /// leaves such events listed; the caller must treat the list as faulted.
    pub const fn out_of_order(&self) -> bool {
        self.out_of_order
    }
}

/// Executor core for hardware list zero.
pub(crate) struct SchedulerExecutor<I, const CAPACITY: usize> {
    list: SchedulerList<I, CAPACITY>,
    transaction: Option<Transaction<I, CAPACITY>>,
    stopped: Option<BluetoothSchedulerStopped>,
}

/// The single list transaction in progress.
#[derive(Clone, Copy)]
enum Transaction<I, const CAPACITY: usize> {
    Live(live::LivePhase<I>),
    Cancel(cancel::CancelPhase<I, CAPACITY>),
    Flush,
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerExecutor<I, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            list: SchedulerList::new(),
            transaction: None,
            stopped: None,
        }
    }

    /// The ordered mirror of the hardware list.
    pub const fn list(&self) -> &SchedulerList<I, CAPACITY> {
        &self.list
    }

    /// Insert `id` while the scheduler is idle.
    ///
    /// The mirror decides the position before any item memory changes. The
    /// new item and its neighbours are then linked, and the list head is
    /// returned for publication.
    pub fn submit_idle(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        scheduler: &BluetoothSchedulerBusyObservation,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerIdleInsertion, SchedulerSubmitError<I>> {
        self.admit_transaction()?;
        if self.stopped.is_some() {
            return Err(SchedulerSubmitError::Stopped);
        }
        if scheduler.is_busy() {
            return Err(SchedulerSubmitError::SchedulerBusy);
        }
        self.link_into_list(items, id, window)
            .map_err(SchedulerSubmitError::List)?;
        let (head, _) = self.list.head().expect("the list holds the inserted event");
        Ok(SchedulerIdleInsertion {
            head: items.link(head),
        })
    }

    /// Take the events that hardware executed from the head of the list.
    ///
    /// Hardware has already advanced its head past them, so no register
    /// changes. The new head loses its previous link.
    pub fn take_completed(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
    ) -> Result<SchedulerCompletion<I, CAPACITY>, SchedulerTransactionActive> {
        if self.transaction.is_some() {
            return Err(SchedulerTransactionActive);
        }
        let scan = self
            .list
            .scan_completion(|id| items.completion_status(id).is_some());
        let mut completion = SchedulerCompletion {
            completed: [None; CAPACITY],
            len: 0,
            out_of_order: scan.out_of_order,
        };
        for _ in 0..scan.completed_prefix {
            let (id, _) = self.list.head().expect("the scan counted a listed head");
            let status = items
                .completion_status(id)
                .expect("the scan found this event executed");
            self.list.remove(id);
            completion.completed[completion.len] = Some((id, status));
            completion.len += 1;
        }
        if completion.len > 0
            && let Some((head, _)) = self.list.head()
        {
            items.set_previous(head, None);
        }
        Ok(completion)
    }

    /// Hardware action that restarts an idle scheduler at the first listed
    /// event that has not executed, as insertion end does.
    pub fn restart_if_idle(
        &self,
        items: &impl SchedulerItemAccess<I>,
        scheduler: &BluetoothSchedulerBusyObservation,
    ) -> Option<SchedulerIdleInsertion> {
        if scheduler.is_busy() || self.transaction.is_some() || self.stopped.is_some() {
            return None;
        }
        self.first_unexecuted(items)
    }

    pub(crate) fn cancellation_items(&self) -> impl Iterator<Item = I> + '_ {
        self.transaction
            .iter()
            .filter_map(|transaction| match transaction {
                Transaction::Cancel(phase) => Some(phase),
                _ => None,
            })
            .flat_map(|phase| phase.detached_ids())
    }

    /// Advance the list transaction in progress with the awaited observation.
    pub fn advance(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        observation: SchedulerObservation,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerTransactionFault> {
        match self
            .transaction
            .ok_or(SchedulerTransactionFault::NoTransaction)?
        {
            Transaction::Live(phase) => self.advance_live(items, phase, observation),
            Transaction::Cancel(phase) => self.advance_cancel(items, phase, observation),
            Transaction::Flush => self.advance_flush(items, observation),
        }
    }

    /// Take ownership of the stopped receipt produced by the scheduler stop
    /// sequence.
    ///
    /// While the executor holds it, nothing starts the scheduler: idle
    /// insertion and restart are refused. Completion, cancellation and list
    /// deletion stay available and take their idle paths.
    pub fn enter_stopped(
        &mut self,
        stopped: BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerStopRejected> {
        if self.transaction.is_some() {
            return Err(SchedulerStopRejected::TransactionActive(stopped));
        }
        if self.stopped.is_some() {
            return Err(SchedulerStopRejected::AlreadyStopped(stopped));
        }
        self.stopped = Some(stopped);
        Ok(())
    }

    /// The stopped receipt the executor holds.
    pub const fn stopped(&self) -> Option<&BluetoothSchedulerStopped> {
        self.stopped.as_ref()
    }

    /// Consume the stopped receipt and report the event to restart at.
    ///
    /// Listed events keep their windows; the caller cancels events whose
    /// start has passed before it resumes.
    pub fn resume(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
    ) -> Result<Option<SchedulerIdleInsertion>, SchedulerNotStopped> {
        // Resuming ends the stopped span; the receipt is spent.
        drop(self.stopped.take().ok_or(SchedulerNotStopped)?);
        Ok(self.first_unexecuted(items))
    }

    fn first_unexecuted(
        &self,
        items: &impl SchedulerItemAccess<I>,
    ) -> Option<SchedulerIdleInsertion> {
        self.list
            .iter()
            .map(|(id, _)| id)
            .find(|id| items.completion_status(*id).is_none())
            .map(|head| SchedulerIdleInsertion {
                head: items.link(head),
            })
    }

    fn admit_transaction(&self) -> Result<(), SchedulerSubmitError<I>> {
        if self.transaction.is_some() {
            return Err(SchedulerSubmitError::TransactionActive);
        }
        Ok(())
    }

    /// Link `id` into the mirror and the item memory.
    fn link_into_list(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<(), SchedulerListInsertError<I>> {
        let placement = self.list.insert(id, window)?;
        let link = items.link(id);
        let previous = placement.predecessor.map(|previous| items.link(previous));
        let next = placement.successor.map(|next| items.link(next));
        items.prepare_for_list(id, previous, next);
        if let Some(predecessor) = placement.predecessor {
            items.set_next(predecessor, Some(link));
        }
        if let Some(successor) = placement.successor {
            items.set_previous(successor, Some(link));
        }
        Ok(())
    }
}

impl<I: Copy + Eq, const CAPACITY: usize> Default for SchedulerExecutor<I, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

mod cancel;
mod live;
mod step;

pub use cancel::SchedulerCancelError;
pub use step::{
    SchedulerNext, SchedulerObservation, SchedulerReleased, SchedulerStep,
    SchedulerTransactionFault, SchedulerWait,
};

#[cfg(test)]
mod tests;

pub(crate) use step::SchedulerAction;
