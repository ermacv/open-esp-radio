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
//! serialized with interrupt service, and the caller owns the deadline.

use oer_esp32s31_pac::{
    BluetoothInterruptRegisters, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerExecutionModifyPublished, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerInsertionCommand, BluetoothTaskRegisters,
};

use super::diagnostic::{self, BluetoothDiagnosticReadBudget, BluetoothDiagnosticUnsettled};

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
    type Published;

    fn busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled>;
    /// Command-zero status 26 then, only if set, command-one status 18.
    fn engines_idle(&mut self) -> bool;
    fn select_progress(&mut self);
    fn publish(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        list_deletion: bool,
    ) -> Self::Published;
    fn repeats(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled>;
    fn ready(&mut self) -> bool;
    fn select_settle(&mut self);
    fn settled(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled>;
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

fn step<C: Control>(
    state: State<'_, C::Published>,
    hw: &mut C,
) -> Result<BluetoothSchedulerExecutionModifyDisposition, BluetoothDiagnosticUnsettled> {
    use BluetoothSchedulerExecutionModifyDisposition::{HardwareRejected, Pending, Ready};
    loop {
        match *state.phase {
            Phase::Preamble => {
                if hw.busy()? && !hw.engines_idle() {
                    return Ok(Pending);
                }
                hw.select_progress();
                *state.published = Some(hw.publish(state.index, state.list_deletion));
                *state.phase = Phase::Progress { selected: true };
            }
            Phase::Progress { selected } => {
                if !selected {
                    hw.select_progress();
                }
                if hw.repeats()? {
                    *state.repeat = true;
                    *state.phase = Phase::Progress { selected: false };
                    return Ok(Pending);
                }
                *state.phase = Phase::Completion;
            }
            Phase::Completion => {
                if hw.busy()? && !hw.ready() {
                    return Ok(Pending);
                }
                *state.phase = Phase::Settle { selected: false };
            }
            Phase::Settle { selected } => {
                if !selected {
                    hw.select_settle();
                }
                if !hw.settled()? {
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

struct Hardware<'a> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut BluetoothInterruptRegisters,
    budget: BluetoothDiagnosticReadBudget,
}

#[allow(
    unsafe_code,
    reason = "the admitted request carries the caller's list ownership and serialization"
)]
impl Control for Hardware<'_> {
    type Published = BluetoothSchedulerExecutionModifyPublished;

    fn busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        diagnostic::settle(self.budget, || self.interrupts.capture_scheduler_busy())
            .map(|busy| busy.is_busy())
    }
    fn engines_idle(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    fn select_progress(&mut self) {
        self.task
            .select_scheduler_execution_modify_progress(self.interrupts);
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
    fn repeats(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        diagnostic::settle(self.budget, || {
            self.task
                .scheduler_execution_modify_repeats(self.interrupts)
        })
    }
    fn ready(&mut self) -> bool {
        self.task.scheduler_execution_modify_ready()
    }
    fn select_settle(&mut self) {
        self.task
            .select_scheduler_execution_modify_settle(self.interrupts);
    }
    fn settled(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        diagnostic::settle(self.budget, || {
            self.task
                .scheduler_execution_modify_settled(self.interrupts)
        })
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

/// Advance one finite execution-modify step while both register owners are
/// held.
pub(crate) fn step_hardware(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut BluetoothInterruptRegisters,
    modify: &mut BluetoothSchedulerExecutionModify,
    budget: BluetoothDiagnosticReadBudget,
) -> Result<BluetoothSchedulerExecutionModifyDisposition, BluetoothDiagnosticUnsettled> {
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
            budget,
        },
    )
}

#[cfg(test)]
mod tests;
