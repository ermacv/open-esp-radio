//! Finished-list notifications for the task service. Address publication belongs to HAL.

use super::ControllerPoweredTaskRuntime;
use crate::{interrupt::SchedulerWakeBatch, scheduler::SchedulerFinishedListCaptureError};
use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;
use oer_espressif_interrupt_table::signal::SignalExecutor;
impl<X: SignalExecutor, M: ControllerMemory> ControllerPoweredTaskRuntime<'_, X, M> {
    /// Capture one fenced finished-list transfer into this epoch's worker.
    pub fn capture_scheduler_finished_lists(
        &mut self,
        wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        let controller = self.task.engine();
        self.runtime
            .scheduler_finished_lists
            .capture(controller, wake)
    }

    /// Capture the finished lists of a stopped scheduler into this epoch's
    /// worker.
    pub fn capture_stopped_scheduler_finished_lists(
        &mut self,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        let controller = self.task.engine();
        self.runtime
            .scheduler_finished_lists
            .capture_stopped(controller)
    }
}
