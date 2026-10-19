//! Cancellation of listed events and deletion of the whole list.
//!
//! Cancellation follows the vendor two-phase deletion. First every cancelled
//! event is marked deleted and unlinked from the mirror and from its
//! neighbours; its own hardware next link stays, so hardware that already
//! holds it can still follow the chain. With the scheduler idle, the list
//! head is then republished and the events return at once.
//!
//! With the scheduler running, the executor waits for the lock-modify
//! request to be idle and opens the cancellation hold. Each detached event
//! that has not executed and does not start after the hardware head sampled
//! at the start is then skipped one at a time. Results one and three
//! continue, result two and an idle scheduler end the loop, and the hold
//! closes. The events return when the hold is released. The vendor opens the
//! hold only when its environment enables the lock-modify request; this
//! executor always uses that request for live insertion, so it always opens
//! the hold.
//!
//! List deletion clears the list head. A running scheduler first grants
//! execution modify in its list-deletion mode.

use oer_esp32s31_bluetooth_memory::ControllerSramLinkAddress;
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerCancellationDisposition,
    BluetoothSchedulerExecutionModifyDisposition, BluetoothSchedulerSkipDisposition,
    BluetoothSchedulerSkipResult,
};

use super::{
    SchedulerAction, SchedulerExecutor, SchedulerItemAccess, SchedulerNext, SchedulerObservation,
    SchedulerReleased, SchedulerStep, SchedulerTransactionActive, SchedulerTransactionFault,
    SchedulerWait, Transaction,
};
use crate::scheduler::window::SchedulerRawWindow;

/// Why a cancellation was not started. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerCancelError<I> {
    /// A list transaction is in progress.
    TransactionActive,
    /// The event is not listed, or it is named twice.
    NotListed(I),
    /// The sampled hardware head is not a listed event, so the skip criterion
    /// cannot be evaluated. The caller samples the head again.
    ForeignHardwareHead,
}

#[derive(Clone, Copy)]
struct Detached<I> {
    id: I,
    skip: bool,
}

#[derive(Clone, Copy)]
enum CancelStage {
    LockModify,
    Hold,
    Skip { at: usize },
}

#[derive(Clone, Copy)]
pub(super) struct CancelPhase<I, const CAPACITY: usize> {
    detached: [Option<Detached<I>>; CAPACITY],
    len: usize,
    stage: CancelStage,
}

impl<I: Copy + Eq, const CAPACITY: usize> CancelPhase<I, CAPACITY> {
    fn detached(&self) -> impl Iterator<Item = Detached<I>> + '_ {
        self.detached[..self.len].iter().flatten().copied()
    }
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerExecutor<I, CAPACITY> {
    /// Cancel listed events.
    ///
    /// `hardware_head` is the list head sampled after `scheduler`; it is
    /// consulted only while the scheduler is busy. Every event is validated
    /// before any item memory changes.
    pub fn begin_cancel(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        scheduler: &BluetoothSchedulerBusyObservation,
        hardware_head: Option<ControllerSramLinkAddress>,
        ids: &[I],
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerCancelError<I>> {
        if self.transaction.is_some() {
            return Err(SchedulerCancelError::TransactionActive);
        }
        for (index, id) in ids.iter().enumerate() {
            if !self.list.contains(*id) || ids[..index].contains(id) {
                return Err(SchedulerCancelError::NotListed(*id));
            }
        }
        let head_start = match (scheduler.is_busy(), hardware_head) {
            (true, Some(head)) => Some(
                self.list
                    .iter()
                    .find(|(id, _)| items.link(*id) == head)
                    .map(|(_, window)| window.start())
                    .ok_or(SchedulerCancelError::ForeignHardwareHead)?,
            ),
            _ => None,
        };

        let mut phase = CancelPhase {
            detached: [None; CAPACITY],
            len: 0,
            stage: CancelStage::LockModify,
        };
        let mut cancelled = [None; CAPACITY];
        for (slot, entry) in cancelled
            .iter_mut()
            .zip(self.list.iter().filter(|(id, _)| ids.contains(id)))
        {
            *slot = Some(entry);
        }
        for (id, window) in cancelled.into_iter().flatten() {
            self.detach(items, id);
            phase.detached[phase.len] = Some(Detached {
                id,
                skip: starts_at_or_before(window, head_start),
            });
            phase.len += 1;
        }

        if !scheduler.is_busy() {
            let head = self.list.head().map(|(id, _)| items.link(id));
            return Ok(SchedulerStep::finished(
                &[SchedulerAction::PublishHead(head)],
                release(items, phase.detached().map(|detached| detached.id)),
            ));
        }
        self.transaction = Some(Transaction::Cancel(phase));
        Ok(SchedulerStep::wait(SchedulerWait::LockModify))
    }

    /// Delete every listed event.
    pub fn begin_flush(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        scheduler: &BluetoothSchedulerBusyObservation,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerTransactionActive> {
        if self.transaction.is_some() {
            return Err(SchedulerTransactionActive);
        }
        if !scheduler.is_busy() {
            return Ok(SchedulerStep::finished(
                &[SchedulerAction::PublishHead(None)],
                self.release_all(items),
            ));
        }
        self.transaction = Some(Transaction::Flush);
        Ok(SchedulerStep::new(
            &[SchedulerAction::PublishExecutionModifyListDeletion],
            SchedulerNext::Await(SchedulerWait::ExecutionModify),
        ))
    }

    pub(super) fn advance_cancel(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        mut phase: CancelPhase<I, CAPACITY>,
        observation: SchedulerObservation,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerTransactionFault> {
        match (phase.stage, observation) {
            (CancelStage::LockModify, SchedulerObservation::LockModify(observation)) => {
                if observation.wait_active() {
                    return Ok(SchedulerStep::wait(SchedulerWait::LockModify));
                }
                phase.stage = CancelStage::Hold;
                self.transaction = Some(Transaction::Cancel(phase));
                Ok(SchedulerStep::new(
                    &[
                        SchedulerAction::IndexCancellation,
                        SchedulerAction::AcknowledgeCancellationSource,
                        SchedulerAction::RequestCancellation,
                    ],
                    SchedulerNext::Await(SchedulerWait::Cancellation),
                ))
            }
            (CancelStage::Hold, SchedulerObservation::Cancellation(disposition)) => {
                match disposition {
                    BluetoothSchedulerCancellationDisposition::Pending => {
                        Ok(SchedulerStep::wait(SchedulerWait::Cancellation))
                    }
                    BluetoothSchedulerCancellationDisposition::Settled
                    | BluetoothSchedulerCancellationDisposition::SchedulerIdle => {
                        Ok(self.next_skip(items, phase, 0, None))
                    }
                }
            }
            (CancelStage::Skip { at }, SchedulerObservation::Skip(disposition)) => {
                match disposition {
                    BluetoothSchedulerSkipDisposition::Pending => {
                        Ok(SchedulerStep::wait(SchedulerWait::Skip))
                    }
                    BluetoothSchedulerSkipDisposition::Completed(
                        BluetoothSchedulerSkipResult::One | BluetoothSchedulerSkipResult::Three,
                    ) => Ok(self.next_skip(items, phase, at + 1, Some(SchedulerAction::ClearSkip))),
                    BluetoothSchedulerSkipDisposition::Completed(
                        BluetoothSchedulerSkipResult::Two,
                    )
                    | BluetoothSchedulerSkipDisposition::SchedulerIdle => {
                        Ok(self.finish_cancel(items, &phase, &[SchedulerAction::ClearSkip]))
                    }
                    BluetoothSchedulerSkipDisposition::UnsupportedHardwareResult => {
                        self.transaction = None;
                        Err(SchedulerTransactionFault::UnsupportedSkipResult)
                    }
                }
            }
            _ => Err(SchedulerTransactionFault::UnexpectedObservation),
        }
    }

    pub(super) fn advance_flush(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        observation: SchedulerObservation,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerTransactionFault> {
        let SchedulerObservation::ExecutionModify(disposition) = observation else {
            return Err(SchedulerTransactionFault::UnexpectedObservation);
        };
        match disposition {
            BluetoothSchedulerExecutionModifyDisposition::Pending => {
                Ok(SchedulerStep::wait(SchedulerWait::ExecutionModify))
            }
            BluetoothSchedulerExecutionModifyDisposition::Ready => {
                self.transaction = None;
                Ok(SchedulerStep::finished(
                    &[
                        SchedulerAction::PublishHead(None),
                        SchedulerAction::ReleaseExecutionModify,
                    ],
                    self.release_all(items),
                ))
            }
            BluetoothSchedulerExecutionModifyDisposition::HardwareRejected => {
                self.transaction = None;
                Err(SchedulerTransactionFault::ExecutionModifyRejected)
            }
        }
    }

    /// Skip the next detached event from `from` that is still unexecuted, or
    /// close the hold. Execution is sampled here, as the vendor loop does.
    fn next_skip(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        mut phase: CancelPhase<I, CAPACITY>,
        from: usize,
        clear: Option<SchedulerAction>,
    ) -> SchedulerStep<I, CAPACITY> {
        let clear = clear.as_slice();
        let next = phase
            .detached()
            .enumerate()
            .skip(from)
            .find(|(_, detached)| detached.skip && items.completion_status(detached.id).is_none());
        let Some((at, detached)) = next else {
            return self.finish_cancel(items, &phase, clear);
        };
        phase.stage = CancelStage::Skip { at };
        self.transaction = Some(Transaction::Cancel(phase));
        let mut actions = [SchedulerAction::ClearSkip; 2];
        actions[clear.len()] = SchedulerAction::PublishSkip(items.link(detached.id));
        SchedulerStep::new(
            &actions[..=clear.len()],
            SchedulerNext::Await(SchedulerWait::Skip),
        )
    }

    fn finish_cancel(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        phase: &CancelPhase<I, CAPACITY>,
        clear: &[SchedulerAction],
    ) -> SchedulerStep<I, CAPACITY> {
        self.transaction = None;
        let mut actions = [SchedulerAction::ReleaseCancellation; 2];
        actions[..clear.len()].copy_from_slice(clear);
        SchedulerStep::finished(
            &actions[..=clear.len()],
            release(items, phase.detached().map(|detached| detached.id)),
        )
    }

    /// Mark `id` deleted and unlink it from the mirror and its neighbours.
    fn detach(&mut self, items: &mut impl SchedulerItemAccess<I>, id: I) {
        items.mark_deleted(id);
        let removal = self.list.remove(id).expect("a validated event is listed");
        let previous = removal.predecessor.map(|previous| items.link(previous));
        let next = removal.successor.map(|next| items.link(next));
        if let Some(predecessor) = removal.predecessor {
            items.set_next(predecessor, next);
        }
        if let Some(successor) = removal.successor {
            items.set_previous(successor, previous);
        }
    }

    fn release_all(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
    ) -> SchedulerReleased<I, CAPACITY> {
        let released = release(items, self.list.iter().map(|(id, _)| id));
        self.list = super::SchedulerList::new();
        released
    }
}

/// Whether an event starts no later than the sampled hardware head. Without
/// a head, the vendor skips every unexecuted event.
fn starts_at_or_before(window: SchedulerRawWindow, head_start: Option<u32>) -> bool {
    head_start.is_none_or(|head| (window.start().wrapping_sub(head) as i32) <= 0)
}

fn release<I: Copy, const CAPACITY: usize>(
    items: &impl SchedulerItemAccess<I>,
    ids: impl Iterator<Item = I>,
) -> SchedulerReleased<I, CAPACITY> {
    let mut released = SchedulerReleased::new();
    for id in ids {
        released.push(id, items.completion_status(id));
    }
    released
}
