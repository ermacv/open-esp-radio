//! Finite execution-modify sequence from the reviewed common-library body.
//!
//! SOURCE: pinned `libbtdm_common.a` member `18.o`
//! `r_sym_bt_rPoPGH6BBYjZaunDU5FV` (the `r_btdm_sched_execution_modify`
//! role). Each attempt waits until the scheduler is idle or both command
//! engines report idle, selects the progress diagnostic and publishes the
//! request. While the progress signal reports a conflicting state the attempt
//! is marked for repetition. It then waits while the scheduler is busy and
//! the request is not ready, and until the settle signal differs from 9. A
//! marked attempt clears START and starts over. The vendor runs the attempts
//! in one critical section with polling loops; here every step is finite,
//! sequenced through durable P3 replies, and the caller owns the deadline.

use oer_esp32s31_pac::{
    BluetoothSchedulerExecutionModifyDisposition, BluetoothSchedulerExecutionModifyPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerInsertionCommand,
    BluetoothTaskRegisters,
};

#[cfg(feature = "bluetooth-engine")]
use super::engine::scheduler::interrupt::{
    SchedulerDoorbell, SchedulerInterruptError, SchedulerTaskHalf,
};
#[cfg(test)]
use oer_time::settle::Unsettled;

#[derive(Debug)]
enum Phase {
    /// Before the request: the engines must be idle.
    Preamble,
    /// The progress signal may still report a conflict.
    Progress { selected: bool },
    /// Waiting for the request while the scheduler is busy.
    Completion,
    /// Waiting for the settle signal to leave 9.
    Settle { selected: bool },
}

/// One execution-modify request of one hardware list and its progress.
#[derive(Debug)]
#[must_use = "an admitted execution modify must be stepped and its START cleared"]
pub struct BluetoothSchedulerExecutionModify {
    index: BluetoothSchedulerHardwareListIndex,
    list_deletion: bool,
    phase: Phase,
    repeat: bool,
    published: Option<BluetoothSchedulerExecutionModifyPublished>,
}

impl BluetoothSchedulerExecutionModify {
    pub(crate) const fn new(
        index: BluetoothSchedulerHardwareListIndex,
        list_deletion: bool,
    ) -> Self {
        Self {
            index,
            list_deletion,
            phase: Phase::Preamble,
            repeat: false,
            published: None,
        }
    }

    /// The published command, whose START its owner clears.
    pub(crate) fn take_published(&mut self) -> Option<BluetoothSchedulerExecutionModifyPublished> {
        self.published.take()
    }
}

trait Control {
    type Error;
    type Published;

    async fn busy(&mut self) -> Result<bool, Self::Error>;
    /// Command-zero status 26 then, only if set, command-one status 18.
    fn engines_idle(&mut self) -> bool;
    async fn select_progress(&mut self) -> Result<(), Self::Error>;
    fn publish(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        list_deletion: bool,
    ) -> Self::Published;
    async fn repeats(&mut self) -> Result<bool, Self::Error>;
    fn ready(&mut self) -> bool;
    async fn select_settle(&mut self) -> Result<(), Self::Error>;
    async fn settled(&mut self) -> Result<bool, Self::Error>;
    fn clear_start(&mut self, published: Self::Published);
    fn rejected(&mut self) -> bool;
}

struct State<'a, P> {
    index: BluetoothSchedulerHardwareListIndex,
    list_deletion: bool,
    phase: &'a mut Phase,
    repeat: &'a mut bool,
    published: &'a mut Option<P>,
}

async fn step<C: Control>(
    state: State<'_, C::Published>,
    hw: &mut C,
) -> Result<BluetoothSchedulerExecutionModifyDisposition, C::Error> {
    use BluetoothSchedulerExecutionModifyDisposition::{HardwareRejected, Pending, Ready};
    loop {
        match *state.phase {
            Phase::Preamble => {
                if hw.busy().await? && !hw.engines_idle() {
                    return Ok(Pending);
                }
                hw.select_progress().await?;
                *state.published = Some(hw.publish(state.index, state.list_deletion));
                *state.phase = Phase::Progress { selected: true };
            }
            Phase::Progress { selected } => {
                if !selected {
                    hw.select_progress().await?;
                }
                if hw.repeats().await? {
                    *state.repeat = true;
                    *state.phase = Phase::Progress { selected: false };
                    return Ok(Pending);
                }
                *state.phase = Phase::Completion;
            }
            Phase::Completion => {
                if hw.busy().await? && !hw.ready() {
                    return Ok(Pending);
                }
                *state.phase = Phase::Settle { selected: false };
            }
            Phase::Settle { selected } => {
                if !selected {
                    hw.select_settle().await?;
                }
                if !hw.settled().await? {
                    *state.phase = Phase::Settle { selected: false };
                    return Ok(Pending);
                }
                if core::mem::take(state.repeat) {
                    let published = state
                        .published
                        .take()
                        .expect("a settled request was published");
                    hw.clear_start(published);
                    *state.phase = Phase::Preamble;
                    continue;
                }
                return Ok(if hw.rejected() {
                    HardwareRejected
                } else {
                    Ready
                });
            }
        }
    }
}

#[cfg(feature = "bluetooth-engine")]
struct Hardware<'a, D> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut SchedulerTaskHalf,
    bell: &'a D,
}

#[allow(
    unsafe_code,
    reason = "the admitted request carries the caller's list ownership and serialization"
)]
#[cfg(feature = "bluetooth-engine")]
impl<D: SchedulerDoorbell> Control for Hardware<'_, D> {
    type Error = SchedulerInterruptError;
    type Published = BluetoothSchedulerExecutionModifyPublished;

    async fn busy(&mut self) -> Result<bool, SchedulerInterruptError> {
        self.interrupts
            .busy(self.bell)
            .await
            .map(|busy| busy.is_busy())
    }
    fn engines_idle(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    async fn select_progress(&mut self) -> Result<(), Self::Error> {
        self.interrupts.select_progress(self.bell).await
    }
    fn publish(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        list_deletion: bool,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        // SAFETY: `admit_scheduler_execution_modify` took the caller's list
        // ownership and serialization contract for this exact request.
        unsafe {
            if list_deletion {
                self.task
                    .publish_scheduler_execution_modify_list_deletion(index)
            } else {
                self.task.publish_scheduler_execution_modify(index)
            }
        }
    }
    async fn repeats(&mut self) -> Result<bool, SchedulerInterruptError> {
        self.interrupts.progress(self.bell).await
    }
    fn ready(&mut self) -> bool {
        self.task.scheduler_execution_modify_ready()
    }
    async fn select_settle(&mut self) -> Result<(), Self::Error> {
        self.interrupts.select_settle(self.bell).await
    }
    async fn settled(&mut self) -> Result<bool, SchedulerInterruptError> {
        self.interrupts.settled(self.bell).await
    }
    fn clear_start(&mut self, _published: BluetoothSchedulerExecutionModifyPublished) {
        // SAFETY: the consumed proof is the command this clear ends.
        let _cleared = unsafe {
            self.task
                .clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::One)
        };
    }
    fn rejected(&mut self) -> bool {
        self.task.scheduler_execution_modify_rejected()
    }
}

/// Advance one finite execution-modify step through the unique task request endpoint.
#[cfg(feature = "bluetooth-engine")]
pub(crate) async fn step_hardware<D: SchedulerDoorbell>(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut SchedulerTaskHalf,
    modify: &mut BluetoothSchedulerExecutionModify,
    bell: &D,
) -> Result<BluetoothSchedulerExecutionModifyDisposition, SchedulerInterruptError> {
    step(
        State {
            index: modify.index,
            list_deletion: modify.list_deletion,
            phase: &mut modify.phase,
            repeat: &mut modify.repeat,
            published: &mut modify.published,
        },
        &mut Hardware {
            task,
            interrupts,
            bell,
        },
    )
    .await
}

#[cfg(test)]
mod tests;
