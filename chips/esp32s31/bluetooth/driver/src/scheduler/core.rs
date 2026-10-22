//! Fact-bounded scheduler initialization after the controller HAL component.

use oer_esp32s31_hal::bluetooth::engine::scheduler::SchedulerSoftwareConfig;
use oer_interrupt_table::signal::SignalExecutor;

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
pub struct SchedulerInitialized<'cells, const MODEM_TIMER_CAPACITY: usize, X: SignalExecutor> {
    task: TaskResources,
    interrupts: Option<InterruptBankOwner>,
    time_scale: BluetoothControllerTimeScale,
    standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    config: SchedulerSoftwareConfig,
    _hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY, X>,
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize, X: SignalExecutor>
    SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY, X>
{
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
    ///
    /// # Errors
    ///
    /// The caller runs outside the event cells' executor.
    pub fn runtime_is_pristine(&self) -> Result<bool, oer_interrupt_table::signal::ForeignContext> {
        self.runtime.is_pristine()
    }

    /// Hand the task owner and the software workers to their endpoints.
    ///
    /// The task endpoint owns the task-side HAL owner and the scheduler
    /// workers; the interrupt endpoint borrows only the shared event cells.
    /// The interrupt bank must already have left this owner.
    pub(crate) fn split_runtime(
        self,
    ) -> (
        ControllerInterruptRuntime<'cells, X>,
        ControllerPoweredTaskRuntime<'cells, X>,
        ControllerModemTimerRuntime<'cells, MODEM_TIMER_CAPACITY, X>,
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

/// A short exclusive view of the durable controller-time phase.
#[cfg(target_arch = "riscv32")]
pub enum ControllerTimeAccess<'owner, 'cells, X: SignalExecutor> {
    Idle(ControllerTimeReady<'owner, 'cells, X>),
    Requested,
    DrainingOrphan,
    Faulted,
}

/// Permission to publish one request, valid only for this task's idle phase.
///
/// ```compile_fail,E0382
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// use oer_esp32s31_bluetooth_driver::scheduler::ControllerTimeReady;
/// fn duplicate<X: oer_interrupt_table::signal::SignalExecutor>(ready: ControllerTimeReady<'_, '_, X>) {
///     let first = ready.request();
///     let duplicate = ready.request();
/// }
/// ```
#[cfg(target_arch = "riscv32")]
pub struct ControllerTimeReady<'owner, 'cells, X: SignalExecutor> {
    runtime: &'owner mut ControllerPoweredTaskRuntime<'cells, X>,
}

#[cfg(target_arch = "riscv32")]
impl<X: SignalExecutor> ControllerTimeReady<'_, '_, X> {
    pub fn request(
        self,
    ) -> Result<
        crate::controller_time::ControllerTimeRequest,
        crate::controller_time::ControllerTimeRequestError,
    > {
        self.runtime
            ._standalone_dtm_profile
            .gate_controller_time_request();
        self.runtime.task.request_idle_controller_time()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'cells, X: SignalExecutor> ControllerPoweredTaskRuntime<'cells, X> {
    #[cfg(target_arch = "riscv32")]
    pub fn controller_time_access(&mut self) -> ControllerTimeAccess<'_, 'cells, X> {
        use crate::controller_time::ControllerTimeWorkerPhase as Phase;
        match self.task.controller_time_phase() {
            Phase::Idle => ControllerTimeAccess::Idle(ControllerTimeReady { runtime: self }),
            Phase::Requested => ControllerTimeAccess::Requested,
            Phase::DrainingOrphan => ControllerTimeAccess::DrainingOrphan,
            Phase::Faulted => ControllerTimeAccess::Faulted,
        }
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

/// Rejection before any scheduler hardware mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerInitializationError {
    RuntimeNotPristine,
    ForeignContext,
}
/// Exact input owners retained when scheduler initialization is refused.
#[must_use = "a refused initialization retains both input owners"]
pub struct SchedulerInitializationFailure<'cells, const MT: usize, X: SignalExecutor> {
    pub error: SchedulerInitializationError,
    pub controller: ControllerHalInitialized,
    pub runtime: ControllerRuntimeResources<'cells, MT, X>,
}
impl<const MT: usize, X: SignalExecutor> core::fmt::Debug
    for SchedulerInitializationFailure<'_, MT, X>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SchedulerInitializationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl ControllerHalInitialized {
    /// Initialize scheduler hardware and bind one static no-RTOS runtime.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The supplied runtime must be pristine, observed
    /// in its event cells' executor where the caller runs, and is consumed
    /// into the same powered ownership epoch; it replaces the vendor event,
    /// broker-node and task containers instead of emulating their ABI.
    ///
    /// # Errors
    ///
    /// A non-pristine runtime or foreign executor context returns both input
    /// owners unchanged, before any scheduler hardware write.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_scheduler<'cells, X: SignalExecutor, const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY, X>,
    ) -> Result<
        SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY, X>,
        SchedulerInitializationFailure<'cells, MODEM_TIMER_CAPACITY, X>,
    > {
        self.initialize_scheduler_with(runtime, |task| task.clear_scheduler_hardware_list_heads())
    }

    #[cfg(test)]
    pub fn initialize_scheduler_for_validation<
        'cells,
        X: SignalExecutor,
        const MODEM_TIMER_CAPACITY: usize,
    >(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY, X>,
    ) -> Result<
        SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY, X>,
        SchedulerInitializationFailure<'cells, MODEM_TIMER_CAPACITY, X>,
    > {
        self.initialize_scheduler_with(runtime, |_| {
            BluetoothSchedulerHardwareListsCleared::for_host_test()
        })
    }

    fn initialize_scheduler_with<'cells, X: SignalExecutor, const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<'cells, MODEM_TIMER_CAPACITY, X>,
        initialize_hardware: impl FnOnce(&mut TaskResources) -> BluetoothSchedulerHardwareListsCleared,
    ) -> Result<
        SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY, X>,
        SchedulerInitializationFailure<'cells, MODEM_TIMER_CAPACITY, X>,
    > {
        let error = match runtime.is_pristine() {
            Ok(true) => None,
            Ok(false) => Some(SchedulerInitializationError::RuntimeNotPristine),
            Err(_) => Some(SchedulerInitializationError::ForeignContext),
        };
        if let Some(error) = error {
            return Err(SchedulerInitializationFailure {
                error,
                controller: self,
                runtime,
            });
        }
        let Self {
            mut task,
            interrupts,
            time_scale,
            standalone_dtm_profile,
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        Ok(SchedulerInitialized {
            task,
            interrupts: Some(interrupts),
            time_scale,
            standalone_dtm_profile,
            config: SchedulerSoftwareConfig::reviewed_standalone(),
            _hardware_lists_cleared: hardware_lists_cleared,
            runtime,
        })
    }
}

#[cfg(test)]
mod tests;
