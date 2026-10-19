//! Insertion into a running hardware list.
//!
//! This follows the vendor insertion bracket. With a listed predecessor, the
//! executor first requests the execution lock at that predecessor. A retained
//! lock admits linking the new event and a lock-modify request for it; the
//! lock is released afterwards. Without a predecessor, or when the lock is not
//! retained, the executor requests execution modify for the list, links the
//! new event, publishes it as the hardware head and releases modify. The
//! vendor takes that path when hardware has already reached the predecessor,
//! so every earlier event is running or finished.
//!
//! Every hardware wait is a separate observation; the executor never polls.
//! The effect of each command on execution is a hardware contract that the
//! vendor bodies do not establish, so each result keeps its reviewed
//! classification and an unsupported result ends the insertion without
//! linking the event.

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerExecutionLockDisposition,
    BluetoothSchedulerExecutionModifyDisposition,
};

use super::{
    SchedulerAction, SchedulerExecutor, SchedulerItemAccess, SchedulerNext, SchedulerObservation,
    SchedulerReleased, SchedulerStep, SchedulerSubmitError, SchedulerTransactionFault,
    SchedulerWait, Transaction,
};
use crate::scheduler::window::SchedulerRawWindow;

#[derive(Clone, Copy)]
pub(super) enum LivePhase<I> {
    Lock { id: I, window: SchedulerRawWindow },
    Modify { id: I, window: SchedulerRawWindow },
    LockModify,
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerExecutor<I, CAPACITY> {
    /// Begin inserting `id` into the running list.
    ///
    /// The position is only planned here; nothing is linked until hardware
    /// grants the lock or modify.
    pub fn begin_live_insertion(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        scheduler: &BluetoothSchedulerBusyObservation,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerSubmitError<I>> {
        self.admit_transaction()?;
        if !scheduler.is_busy() {
            return Err(SchedulerSubmitError::SchedulerIdle);
        }
        let placement = self
            .list
            .plan_insert(id, window)
            .map_err(SchedulerSubmitError::List)?;
        Ok(match placement.predecessor {
            Some(predecessor) => {
                self.transaction = Some(Transaction::Live(LivePhase::Lock { id, window }));
                SchedulerStep::new(
                    &[SchedulerAction::PublishExecutionLock(
                        items.link(predecessor),
                    )],
                    SchedulerNext::Await(SchedulerWait::ExecutionLock),
                )
            }
            None => {
                self.transaction = Some(Transaction::Live(LivePhase::Modify { id, window }));
                SchedulerStep::new(
                    &[SchedulerAction::PublishExecutionModify],
                    SchedulerNext::Await(SchedulerWait::ExecutionModify),
                )
            }
        })
    }

    /// Advance the live insertion with the awaited observation.
    pub(super) fn advance_live(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        phase: LivePhase<I>,
        observation: SchedulerObservation,
    ) -> Result<SchedulerStep<I, CAPACITY>, SchedulerTransactionFault> {
        match (phase, observation) {
            (LivePhase::Lock { id, window }, SchedulerObservation::ExecutionLock(disposition)) => {
                match disposition {
                    BluetoothSchedulerExecutionLockDisposition::Pending => {
                        Ok(SchedulerStep::wait(SchedulerWait::ExecutionLock))
                    }
                    BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained => {
                        self.link_planned(items, id, window);
                        self.transaction = Some(Transaction::Live(LivePhase::LockModify));
                        Ok(SchedulerStep::new(
                            &[SchedulerAction::PublishLockModify(items.link(id))],
                            SchedulerNext::Await(SchedulerWait::LockModify),
                        ))
                    }
                    BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead => {
                        self.transaction =
                            Some(Transaction::Live(LivePhase::Modify { id, window }));
                        Ok(SchedulerStep::new(
                            &[
                                SchedulerAction::ReleaseExecutionLock,
                                SchedulerAction::PublishExecutionModify,
                            ],
                            SchedulerNext::Await(SchedulerWait::ExecutionModify),
                        ))
                    }
                    BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult => {
                        self.transaction = None;
                        Err(SchedulerTransactionFault::UnsupportedExecutionLockResult)
                    }
                }
            }
            (
                LivePhase::Modify { id, window },
                SchedulerObservation::ExecutionModify(disposition),
            ) => match disposition {
                BluetoothSchedulerExecutionModifyDisposition::Pending => {
                    Ok(SchedulerStep::wait(SchedulerWait::ExecutionModify))
                }
                BluetoothSchedulerExecutionModifyDisposition::Ready => {
                    self.link_planned(items, id, window);
                    self.transaction = None;
                    Ok(SchedulerStep::finished(
                        &[
                            SchedulerAction::PublishHead(Some(items.link(id))),
                            SchedulerAction::ReleaseExecutionModify,
                        ],
                        SchedulerReleased::new(),
                    ))
                }
                BluetoothSchedulerExecutionModifyDisposition::HardwareRejected => {
                    self.transaction = None;
                    Err(SchedulerTransactionFault::ExecutionModifyRejected)
                }
            },
            (LivePhase::LockModify, SchedulerObservation::LockModify(observation)) => {
                if observation.wait_active() {
                    Ok(SchedulerStep::wait(SchedulerWait::LockModify))
                } else {
                    self.transaction = None;
                    Ok(SchedulerStep::finished(
                        &[SchedulerAction::ReleaseExecutionLock],
                        SchedulerReleased::new(),
                    ))
                }
            }
            _ => Err(SchedulerTransactionFault::UnexpectedObservation),
        }
    }

    /// Link the event planned by `begin_live_insertion`. The mirror is frozen
    /// while the insertion is active, so the planned position still holds.
    fn link_planned(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        id: I,
        window: SchedulerRawWindow,
    ) {
        self.link_into_list(items, id, window)
            .unwrap_or_else(|_| unreachable!("the frozen mirror keeps the planned position"));
    }
}
