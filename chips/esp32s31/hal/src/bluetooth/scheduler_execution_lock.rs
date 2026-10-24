//! Finite execution-lock sequence from the reviewed common-library body.
//!
//! SOURCE: pinned `libbtdm_common.a` member `18.o`
//! `r_sym_bt_9H3AnHbaHJ3auzSvPDme` (the `r_btdm_sched_execution_lock`
//! role). It first runs the idle preamble `r_sym_bt_B8fDByebTHuwRe8FZuRt`,
//! which returns once the scheduler is idle or both command engines report
//! idle, then publishes command zero and observes it until the scheduler is
//! idle or command zero reports its result. The vendor polls inside one
//! critical section; here every step is finite, sequenced through durable P3 replies, and the caller owns the deadline.

use oer_esp32s31_pac::{
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerInsertionCommand,
    BluetoothTaskRegisters,
};

#[cfg(feature = "bluetooth-engine")]
use super::engine::scheduler::interrupt::{
    SchedulerDoorbell, SchedulerInterruptError, SchedulerTaskHalf,
};
#[cfg(test)]
use oer_time::settle::Unsettled;

/// One execution-lock request and its progress.
#[must_use = "an admitted execution lock must be stepped and its START cleared"]
pub struct BluetoothSchedulerExecutionLock {
    request: BluetoothSchedulerExecutionLockRequest,
    published: Option<BluetoothSchedulerExecutionLockPublished>,
}

impl BluetoothSchedulerExecutionLock {
    pub(crate) const fn new(request: BluetoothSchedulerExecutionLockRequest) -> Self {
        Self {
            request,
            published: None,
        }
    }

    /// The published command, whose START its owner clears.
    pub(crate) fn take_published(&mut self) -> Option<BluetoothSchedulerExecutionLockPublished> {
        self.published.take()
    }
}

trait Control {
    type Error;
    type Published;

    async fn busy(&mut self) -> Result<bool, Self::Error>;
    /// Command-zero status 26 then, only if set, command-one status 18.
    fn engines_idle(&mut self) -> bool;
    fn publish(&mut self, request: BluetoothSchedulerExecutionLockRequest) -> Self::Published;
    /// One observation of the published command after a fresh BUSY sample.
    async fn observe(&mut self) -> Result<BluetoothSchedulerExecutionLockDisposition, Self::Error>;
}

async fn step<C: Control>(
    request: BluetoothSchedulerExecutionLockRequest,
    published: &mut Option<C::Published>,
    hw: &mut C,
) -> Result<BluetoothSchedulerExecutionLockDisposition, C::Error> {
    if published.is_none() {
        if hw.busy().await? && !hw.engines_idle() {
            return Ok(BluetoothSchedulerExecutionLockDisposition::Pending);
        }
        *published = Some(hw.publish(request));
    }
    hw.observe().await
}

#[cfg(feature = "bluetooth-engine")]
struct Hardware<'a, D> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut SchedulerTaskHalf,
    bell: &'a D,
}

#[allow(
    unsafe_code,
    reason = "the admitted request carries the caller's item and list serialization"
)]
#[cfg(feature = "bluetooth-engine")]
impl<D: SchedulerDoorbell> Control for Hardware<'_, D> {
    type Error = SchedulerInterruptError;
    type Published = BluetoothSchedulerExecutionLockPublished;

    async fn busy(&mut self) -> Result<bool, SchedulerInterruptError> {
        self.interrupts
            .busy(self.bell)
            .await
            .map(|busy| busy.is_busy())
    }
    fn engines_idle(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    fn publish(
        &mut self,
        request: BluetoothSchedulerExecutionLockRequest,
    ) -> BluetoothSchedulerExecutionLockPublished {
        // SAFETY: `admit_scheduler_execution_lock` took the caller's item and
        // list serialization contract for this exact request.
        unsafe { self.task.publish_scheduler_execution_lock(request) }
    }
    async fn observe(
        &mut self,
    ) -> Result<BluetoothSchedulerExecutionLockDisposition, SchedulerInterruptError> {
        let busy = self.interrupts.busy(self.bell).await?;
        Ok(self.task.observe_scheduler_execution_lock(busy))
    }
}

/// Advance one finite execution-lock step through the unique task request endpoint.
#[cfg(feature = "bluetooth-engine")]
pub(crate) async fn step_hardware<D: SchedulerDoorbell>(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut SchedulerTaskHalf,
    lock: &mut BluetoothSchedulerExecutionLock,
    bell: &D,
) -> Result<BluetoothSchedulerExecutionLockDisposition, SchedulerInterruptError> {
    step(
        lock.request,
        &mut lock.published,
        &mut Hardware {
            task,
            interrupts,
            bell,
        },
    )
    .await
}

/// Clear command-zero START of a published execution lock.
#[allow(unsafe_code, reason = "the consumed request proves the command")]
pub(crate) fn clear_start(
    task: &mut BluetoothTaskRegisters,
    lock: &mut BluetoothSchedulerExecutionLock,
) {
    if lock.take_published().is_some() {
        // SAFETY: the consumed request is the published command this clear
        // ends, and the task runtime serializes the command word.
        let _cleared = unsafe {
            task.clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::Zero)
        };
    }
}

#[cfg(test)]
mod tests;

impl core::fmt::Debug for BluetoothSchedulerExecutionLock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("BluetoothSchedulerExecutionLock")
    }
}
