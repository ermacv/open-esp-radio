//! Bounded event drain for one scheduler finished-list observation.
//!
//! One step consumes at most one hardware-list bit. A caller can therefore
//! return to its executor between steps; this module has no polling loop,
//! allocator, waker or RTOS dependency.

#![forbid(unsafe_code)]

use crate::interrupt::SchedulerWakeBatch;

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerStopped, ControllerHal,
};

pub use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
};

trait SchedulerFinishedListBackend {
    fn transfer_scheduler_finished_lists(&mut self) -> BluetoothSchedulerFinishedListObservation;
}

impl SchedulerFinishedListBackend for ControllerHal<'_> {
    fn transfer_scheduler_finished_lists(&mut self) -> BluetoothSchedulerFinishedListObservation {
        ControllerHal::transfer_scheduler_finished_lists(self)
    }
}

/// Why a fresh task-side finished-list transfer was not admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerFinishedListCaptureError {
    /// A previous captured observation still contains unconsumed list work.
    DrainAlreadyActive,
}

/// Result of one bounded finished-list worker step.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the finished-list step must be retained or dispatched"]
pub enum SchedulerFinishedListWorkerStep {
    /// No captured transfer is active.
    Idle,
    /// The captured observation contained no work or is fully drained.
    Complete,
    /// One hardware list requires a separately proven item-selection step.
    List {
        /// Affine proof for one list from the fenced transfer.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        /// Whether another captured list requires a later bounded step.
        more: bool,
    },
}

/// Durable task-side owner of one captured finished-list observation.
///
/// Capture performs exactly one finite PAC/HAL transfer. Every subsequent
/// [`Self::step`] consumes at most one list and returns to the executor.
pub struct SchedulerFinishedListWorker {
    observation: Option<BluetoothSchedulerFinishedListObservation>,
}

impl SchedulerFinishedListWorker {
    /// Construct an idle worker.
    pub const fn new() -> Self {
        Self { observation: None }
    }

    /// Capture one fresh hardware observation through the unique task-side HAL owner.
    pub fn capture(
        &mut self,
        controller: &mut ControllerHal<'_>,
        wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        self.capture_with(controller, wake)
    }

    /// Capture the finished lists of a stopped scheduler, which raises no
    /// further wake.
    pub fn capture_stopped(
        &mut self,
        controller: &mut ControllerHal<'_>,
        _stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        self.transfer(controller)
    }

    fn capture_with(
        &mut self,
        backend: &mut impl SchedulerFinishedListBackend,
        _wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        self.transfer(backend)
    }

    fn transfer(
        &mut self,
        backend: &mut impl SchedulerFinishedListBackend,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        if self.observation.is_some() {
            return Err(SchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.observation = Some(backend.transfer_scheduler_finished_lists());
        Ok(())
    }

    /// Return at most one affine captured hardware-list observation.
    ///
    /// A finished list is not an item-completion proof. This worker therefore does
    /// not accept a software queue, select a descriptor or change ownership.
    /// The future completed-list owner must match `observed` to an affine
    /// hardware item and inspect the post-fence status before returning it.
    pub fn step(&mut self) -> SchedulerFinishedListWorkerStep {
        let Some(observation) = self.observation.take() else {
            return SchedulerFinishedListWorkerStep::Idle;
        };
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::Complete => {
                SchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => {
                let more = !remaining.is_empty();
                if more {
                    self.observation = Some(remaining);
                }
                SchedulerFinishedListWorkerStep::List { observed, more }
            }
        }
    }

    /// Whether a captured list remains for a later event step.
    pub const fn is_active(&self) -> bool {
        self.observation.is_some()
    }
}

impl Default for SchedulerFinishedListWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
