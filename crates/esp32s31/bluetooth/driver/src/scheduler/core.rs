//! Fact-bounded scheduler initialization after the controller HAL component.

use crate::scheduler::SchedulerSoftwareConfig;

use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerTimeScale, BluetoothSchedulerHardwareListsCleared,
};

use crate::{
    controller_hal::ControllerHalInitialized,
    resources::{InterruptBankOwner, TaskResources},
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
        ControllerRuntimeResources,
    },
};

/// Why one private DTM controller-time phase could not complete.
///
/// Post-enable timing, recurring-RX current, admission and sequence samples are
/// acquired by the Controller and never cross the public DTM preparation
/// boundary. This finite error retains only the logical acquisition outcome;
/// the role-specific preparation failure continues to own every retry resource.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTimeAcquisitionError {
    /// Another request or abandoned request still owns the latch worker.
    Busy,
    /// The logical worker and lower sticky latch owner disagreed at begin.
    OwnershipCollision,
    /// The non-repeating request generation space was exhausted.
    GenerationExhausted,
    /// A recheck or cancellation named a different logical request.
    RequestMismatch,
    /// The lower sticky latch owner disappeared before completion.
    OwnershipLost,
    /// An earlier ownership disagreement stopped the latch worker.
    Faulted,
    /// The caller explicitly abandoned this phase before completion.
    Cancelled,
}

/// Hardware and source-owned software state after scheduler initialization.
///
/// This transition replaces the complete reviewed scheduler-init function:
/// all sixteen hardware list heads are removed, the scheduler policy is
/// retained without copying the vendor structure ABI, and one pristine static
/// Rust runtime replaces the vendor event object and generic broker nodes.
/// Typed event cells and workers make numeric broker source identifiers and an
/// intrusive callback list unnecessary.
///
/// Remaining hardware initialization and stable ISR publication are still
/// missing. This state therefore exposes no PHY, BTBB, IRQ, Controller or
/// Link-Layer readiness. HCI remains outside the
/// hardware boot chain until stable interrupt-owner publication completes.
/// Dropping this state is fail-stop because no verified rollback exists after
/// scheduler MMIO mutation.
#[must_use = "the initialized scheduler retains every powered Bluetooth owner"]
pub struct SchedulerInitialized<'cells, const MODEM_TIMER_CAPACITY: usize> {
    task: TaskResources,
    interrupts: Option<InterruptBankOwner>,
    time_scale: BluetoothControllerTimeScale,
    standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    config: SchedulerSoftwareConfig,
    _hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY>,
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
    pub(crate) fn task_mut(&mut self) -> &mut TaskResources {
        &mut self.task
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn task(&self) -> &TaskResources {
        &self.task
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

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn take_interrupt_owner(&mut self) -> InterruptBankOwner {
        self.interrupts
            .take()
            .expect("private Controller invariant retains the interrupt owner until activation")
    }

    /// Number of fixed modem timer slots retained by the initialized epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.runtime.modem_timer_capacity()
    }

    /// Return the scheduler scale retained by this exact hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Return the source-owned scheduler policy for this hardware epoch.
    pub const fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        self.config
    }

    /// Whether no software event has entered the initialized epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }

    /// Hand the task owner and the software workers to their endpoints.
    ///
    /// The task endpoint owns the task-side HAL owner and the scheduler
    /// workers; the interrupt endpoint borrows only the shared event cells.
    /// The interrupt bank must already have left this owner.
    pub fn split_runtime(
        self,
    ) -> (
        ControllerInterruptRuntime<'cells>,
        ControllerPoweredTaskRuntime<'cells>,
        ControllerModemTimerRuntime<'cells, MODEM_TIMER_CAPACITY>,
    ) {
        assert!(
            self.interrupts.is_none(),
            "the interrupt bank is activated before the runtime splits"
        );
        let (interrupt, software, modem_timer) = self.runtime.split();
        (
            interrupt,
            ControllerPoweredTaskRuntime::new(
                software,
                self.task,
                self.time_scale,
                self.standalone_dtm_profile,
                self.config,
            ),
            modem_timer,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl ControllerPoweredTaskRuntime<'_> {
    #[cfg(target_arch = "riscv32")]
    pub fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::ControllerTimeRequest,
        crate::controller_time::ControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<(), crate::controller_time::ControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.drain_orphan_controller_time()
    }
}

impl ControllerHalInitialized {
    /// Initialize scheduler hardware and bind one static no-RTOS runtime.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The supplied runtime must be pristine and is
    /// consumed into the same powered ownership epoch; it replaces the vendor
    /// event, broker-node and task containers instead of emulating their ABI.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_scheduler<'cells, const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY>,
    ) -> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |task| task.clear_scheduler_hardware_list_heads())
    }

    #[cfg(test)]
    pub fn initialize_scheduler_for_validation<'cells, const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY>,
    ) -> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |_| {
            BluetoothSchedulerHardwareListsCleared::for_validation()
        })
    }

    fn initialize_scheduler_with<'cells, const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY>,
        initialize_hardware: impl FnOnce(&mut TaskResources) -> BluetoothSchedulerHardwareListsCleared,
    ) -> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
        assert!(
            runtime.is_pristine(),
            "only a pristine Controller runtime can initialize a scheduler epoch"
        );
        let Self {
            mut task,
            interrupts,
            time_scale,
            standalone_dtm_profile,
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        SchedulerInitialized {
            task,
            interrupts: Some(interrupts),
            time_scale,
            standalone_dtm_profile,
            config: SchedulerSoftwareConfig::reviewed_standalone(),
            _hardware_lists_cleared: hardware_lists_cleared,
            runtime,
        }
    }
}

#[cfg(test)]
mod tests;
