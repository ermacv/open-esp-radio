//! Fixed no-RTOS software state for one powered Controller epoch.
//!
//! These resources replace independently allocated vendor event, queue, task
//! and generic broker-node objects with one affine Rust owner. They contain no
//! HCI Host state and do not make the radio operational; stable ISR placement
//! and powered hardware transitions remain separate stages.
//!
//! The powered task endpoint executes the scheduler executor's list-zero
//! steps: it performs their hardware actions, takes their wait observations,
//! starts the idle scheduler, captures finished lists and publishes the
//! global receive chains.

#![deny(unsafe_code)]

use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerEpoch;

#[cfg(any(target_arch = "riscv32", test))]
use crate::resources::TaskResources;

use crate::{
    interrupt::SchedulerWakeCell,
    modem_lp_timer_queue::{ModemLpTimerEventCell, ModemLpTimerQueue, ModemLpTimerWorkerWakeCell},
    scheduler::SchedulerFinishedListWorker,
};

#[cfg(target_arch = "riscv32")]
use crate::scheduler::hardware::{SchedulerHardware, live::HalPublications};

/// Event cells shared between the Controller's interrupt handlers and its
/// task-side workers.
///
/// The handlers reach them through stable references, so they live in
/// static storage for the whole boot and serve every Controller epoch. An
/// epoch starts only while no notification is pending.
pub struct ControllerEventCells {
    scheduler_wake: SchedulerWakeCell,
    modem_lp_timer_worker_wake: ModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: ModemLpTimerEventCell,
}

impl ControllerEventCells {
    /// Construct quiet cells without allocation or MMIO.
    pub const fn new() -> Self {
        Self {
            scheduler_wake: SchedulerWakeCell::new(),
            modem_lp_timer_worker_wake: ModemLpTimerWorkerWakeCell::new(),
            modem_lp_timer_events: ModemLpTimerEventCell::new(),
        }
    }

    /// Whether no notification or expiration is pending.
    pub fn is_quiet(&self) -> bool {
        !self.scheduler_wake.is_pending()
            && !self.modem_lp_timer_worker_wake.is_pending()
            && !self.modem_lp_timer_events.is_pending()
    }

    /// Coalesced readiness notifications are not work ownership. Discard
    /// them only after the Controller was reset, with every IRQ route of its
    /// epoch removed, so the next epoch starts quiet.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn clear_notifications_after_reset(&self) {
        let _ = self.scheduler_wake.take();
        let _ = self.modem_lp_timer_worker_wake.take();
    }
}

impl Default for ControllerEventCells {
    fn default() -> Self {
        Self::new()
    }
}

/// Allocation-free event and worker storage for exactly one Controller epoch.
///
/// The workers are owned by value and move with their endpoints; only the
/// interrupt-shared [`ControllerEventCells`] are borrowed. The aggregate is
/// intentionally neither `Copy` nor `Clone`.
#[must_use = "Controller runtime resources must remain owned by their hardware epoch"]
pub struct ControllerRuntimeResources<'cells, const MODEM_TIMER_CAPACITY: usize> {
    cells: &'cells ControllerEventCells,
    scheduler_finished_lists: SchedulerFinishedListWorker,
    #[cfg(target_arch = "riscv32")]
    scheduler_hardware: SchedulerHardware<HalPublications>,
    modem_lp_timer_queue: ModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    modem_lp_timer_epoch: BluetoothModemLpTimerEpoch,
}

/// Shared interrupt-side publications of one Controller epoch.
///
/// This endpoint can only be produced together with the matching
/// [`ControllerTaskRuntime`]. It deliberately exposes no timer queue or
/// scheduler state.
#[must_use = "the interrupt endpoint must remain paired with its task endpoint"]
pub struct ControllerInterruptRuntime<'runtime> {
    scheduler_wake: &'runtime SchedulerWakeCell,
    modem_lp_timer_worker_wake: &'runtime ModemLpTimerWorkerWakeCell,
}

/// Unique mutable modem-timer runtime for one Controller epoch.
///
/// This endpoint is disjoint from both interrupt publication and command-task
/// scheduler ownership. It is kept crate-private at the final composition
/// boundary, where stable source-127 storage is joined to it by the typed modem
/// timer task.
#[must_use = "the modem-timer runtime must remain paired with its Controller epoch"]
pub struct ControllerModemTimerRuntime<'runtime, const MODEM_TIMER_CAPACITY: usize> {
    pub(crate) queue: ModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) epoch: BluetoothModemLpTimerEpoch,
    pub(crate) worker_wake: &'runtime ModemLpTimerWorkerWakeCell,
    pub(crate) events: &'runtime ModemLpTimerEventCell,
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY> {
    /// Borrow the durable source-127 readiness cell without acquiring work.
    pub const fn worker_wake(&self) -> &ModemLpTimerWorkerWakeCell {
        self.worker_wake
    }

    /// Borrow the durable expiration handoff without acquiring timer ownership.
    pub const fn events(&self) -> &ModemLpTimerEventCell {
        self.events
    }

    /// Whether this endpoint still owns an empty software queue.
    pub fn queue_is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl<'runtime> ControllerInterruptRuntime<'runtime> {
    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &'runtime SchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &'runtime ModemLpTimerWorkerWakeCell {
        self.modem_lp_timer_worker_wake
    }
}

/// Task-side events and workers of one Controller epoch.
///
/// The endpoint owns the only workers of its epoch. The scheduler wake is
/// shared with the matching interrupt endpoint returned by the same
/// [`ControllerRuntimeResources::split`] call.
#[must_use = "the task endpoint must remain paired with its interrupt endpoint"]
pub struct ControllerTaskRuntime<'runtime> {
    cells: &'runtime ControllerEventCells,
    scheduler_finished_lists: SchedulerFinishedListWorker,
    #[cfg(target_arch = "riscv32")]
    scheduler_hardware: SchedulerHardware<HalPublications>,
}

impl ControllerTaskRuntime<'_> {
    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        &self.cells.scheduler_wake
    }

    /// The sole bounded finished-list worker for this epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        &mut self.scheduler_finished_lists
    }

    /// Whether scheduler workers hold no transaction that retirement would
    /// lose.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn retirement_ready(&self) -> Result<(), ControllerRuntimeRetirementError> {
        if self.scheduler_finished_lists.is_active() {
            return Err(ControllerRuntimeRetirementError::FinishedLists);
        }
        #[cfg(target_arch = "riscv32")]
        if !self.scheduler_hardware.is_quiet() {
            return Err(ControllerRuntimeRetirementError::ListTransaction);
        }
        Ok(())
    }
}

/// Task-side software and register ownership for one powered Controller epoch.
///
/// This endpoint is produced only by the initialized scheduler lifecycle. It
/// owns the software workers together with the task-side HAL owner, so a live
/// worker step never requires an independently recovered register
/// capability, and [`Self::retire`] returns both for the Controller shutdown.
#[must_use = "the powered task endpoint retains the Controller task owner"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct ControllerPoweredTaskRuntime<'runtime> {
    pub(crate) runtime: ControllerTaskRuntime<'runtime>,
    pub(crate) task: TaskResources,
    pub(crate) time_scale: oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale,
    pub(crate) _standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    pub(crate) config: crate::scheduler::SchedulerSoftwareConfig,
}

/// Why the powered task endpoint cannot be retired yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTaskRetirementError {
    /// A scheduler worker still holds work.
    Runtime(ControllerRuntimeRetirementError),
    /// The controller-time worker holds a request or faulted.
    ControllerTime(crate::controller_time::ControllerTimeRetirementError),
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'runtime> ControllerPoweredTaskRuntime<'runtime> {
    pub(crate) const fn new(
        runtime: ControllerTaskRuntime<'runtime>,
        task: TaskResources,
        time_scale: oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale,
        standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
        config: crate::scheduler::SchedulerSoftwareConfig,
    ) -> Self {
        Self {
            runtime,
            task,
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config,
        }
    }

    /// The HAL task owner of this powered Controller epoch.
    pub fn task_owner(&mut self) -> &mut TaskResources {
        &mut self.task
    }

    /// Return the task owner once no worker and no controller-time request
    /// holds work; the rejection returns the unchanged endpoint.
    #[allow(
        clippy::result_large_err,
        reason = "the rejection returns the complete affine endpoint"
    )]
    pub fn retire(self) -> Result<TaskResources, (ControllerTaskRetirementError, Self)> {
        if let Err(error) = self.runtime.retirement_ready() {
            return Err((ControllerTaskRetirementError::Runtime(error), self));
        }
        if let Err(error) = self.task.controller_time_retirement_ready() {
            return Err((ControllerTaskRetirementError::ControllerTime(error), self));
        }
        Ok(self.task)
    }

    #[cfg(test)]
    pub(crate) fn controller_time_phase(
        &self,
    ) -> crate::controller_time::ControllerTimeWorkerPhase {
        self.task.controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) fn controller_time_needs_recheck(&self) -> bool {
        self.task.controller_time_needs_recheck()
    }

    /// Whether scheduler workers hold no transaction that retirement would lose.
    pub fn runtime_retirement_ready(&self) -> Result<(), ControllerRuntimeRetirementError> {
        self.runtime.retirement_ready()
    }

    /// Scheduler time scale retained by this exact powered Controller epoch.
    pub const fn controller_time_scale(
        &self,
    ) -> oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Source-owned scheduler policy retained by this powered epoch.
    pub const fn scheduler_config(&self) -> crate::scheduler::SchedulerSoftwareConfig {
        self.config
    }

    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// The sole bounded finished-list worker for this powered epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY>
{
    /// Construct one pristine runtime epoch over `cells` without allocation
    /// or MMIO.
    pub const fn new(cells: &'cells ControllerEventCells) -> Self {
        assert!(
            MODEM_TIMER_CAPACITY > 0,
            "a Controller runtime needs at least one modem timer slot"
        );
        Self {
            cells,
            scheduler_finished_lists: SchedulerFinishedListWorker::new(),
            #[cfg(target_arch = "riscv32")]
            scheduler_hardware: SchedulerHardware::new(),
            modem_lp_timer_queue: ModemLpTimerQueue::new(),
            modem_lp_timer_epoch: BluetoothModemLpTimerEpoch::new(),
        }
    }

    /// Number of fixed modem timer slots owned by this epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        MODEM_TIMER_CAPACITY
    }

    /// Whether no event, completion drain, list transaction or timer has
    /// entered this epoch yet.
    pub fn is_pristine(&self) -> bool {
        #[cfg(target_arch = "riscv32")]
        let scheduler_hardware_is_pristine =
            self.scheduler_hardware.is_quiet() && !self.scheduler_hardware.has_run();
        #[cfg(not(target_arch = "riscv32"))]
        let scheduler_hardware_is_pristine = true;

        self.cells.is_quiet()
            && !self.scheduler_finished_lists.is_active()
            && scheduler_hardware_is_pristine
            && self.modem_lp_timer_queue.is_empty()
            && self.modem_lp_timer_epoch.high_byte() == 0
    }

    /// Hand out the only interrupt publisher, task worker and modem-timer
    /// endpoints of this runtime epoch.
    ///
    /// The workers move into their endpoints, so the same workers cannot be
    /// split into a second executor/interrupt pair.
    pub fn split(
        self,
    ) -> (
        ControllerInterruptRuntime<'cells>,
        ControllerTaskRuntime<'cells>,
        ControllerModemTimerRuntime<'cells, MODEM_TIMER_CAPACITY>,
    ) {
        let cells = self.cells;
        (
            ControllerInterruptRuntime {
                scheduler_wake: &cells.scheduler_wake,
                modem_lp_timer_worker_wake: &cells.modem_lp_timer_worker_wake,
            },
            ControllerTaskRuntime {
                cells,
                scheduler_finished_lists: self.scheduler_finished_lists,
                #[cfg(target_arch = "riscv32")]
                scheduler_hardware: self.scheduler_hardware,
            },
            ControllerModemTimerRuntime {
                queue: self.modem_lp_timer_queue,
                epoch: self.modem_lp_timer_epoch,
                worker_wake: &cells.modem_lp_timer_worker_wake,
                events: &cells.modem_lp_timer_events,
            },
        )
    }
}

/// Software work that prevents the Controller shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRuntimeRetirementError {
    /// A captured finished-list set is not drained.
    FinishedLists,
    /// A list transaction holds a hardware publication.
    ListTransaction,
}

#[cfg(target_arch = "riscv32")]
mod scheduler;

#[cfg(target_arch = "riscv32")]
pub use scheduler::ControllerRxChainError;

#[cfg(test)]
mod tests;
