//! Task and interrupt owners of one powered Controller epoch.
//!
//! The epoch starts from the HAL's clocked Bluetooth client, which proves
//! common radio power, the Bluetooth module clocks, the released controller
//! resets and the low-power timer clock. `separate_interrupt_owner` splits
//! it into the task-side [`TaskResources`] and the inactive
//! `InterruptBankOwner`; `TaskResources::shut_down` resets the stopped
//! Controller and returns the clocked client.

#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::power::PlatformClockBackend;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListsCleared;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::bluetooth::InterruptSetupOwner as HalBluetoothInterruptSetupOwner;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::TaskOwner as HalBluetoothTaskOwner;
#[cfg(target_arch = "riscv32")]
use {
    oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerOwnerError,
    oer_esp32s31_hal::bluetooth::ControllerRandomAddress,
    oer_esp32s31_hal::bluetooth::InterruptOutputPreparedOwner,
    oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner,
    oer_esp32s31_hal::domain::RadioDomain,
};

#[cfg(any(target_arch = "riscv32", test))]
use crate::controller_time::ControllerTimeWorker;
#[cfg(any(target_arch = "riscv32", test))]
use crate::controller_time::ControllerTimeWorkerPhase;
#[cfg(target_arch = "riscv32")]
use crate::controller_time::{
    ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequest,
    ControllerTimeRequestError,
};

/// Separate the clocked HAL client into the controller lifecycle's task and
/// IRQ owners without exposing either partition publicly.
///
/// This transition performs no MMIO. In particular it does not configure
/// controller masks or a CPU interrupt route.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) fn separate_interrupt_owner<M: ControllerMemory>(
    engine: oer_esp32s31_hal::bluetooth::engine::Engine<M>,
    interrupts: HalBluetoothInterruptSetupOwner,
) -> (TaskResources<M>, InterruptBankOwner) {
    (
        TaskResources {
            engine,
            controller_time: ControllerTimeWorker::new_idle(),
        },
        InterruptBankOwner {
            _registers: interrupts,
        },
    )
}

/// Ordinary task-side owner of the Bluetooth controller region.
///
/// No MMIO operation is exposed until its finite lifecycle transaction has
/// independent vendor evidence.
#[must_use = "the Bluetooth task owner must return to the clocked client"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct TaskResources<M: ControllerMemory> {
    engine: oer_esp32s31_hal::bluetooth::engine::Engine<M>,
    controller_time: ControllerTimeWorker,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<M: ControllerMemory> TaskResources<M> {
    /// The HAL task owner, which proves the Bluetooth module clocks to the
    /// shared PHY domain and BTBB.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn hal(&self) -> &HalBluetoothTaskOwner {
        self.engine.task_owner()
    }

    /// Reset the stopped Controller, reunite the drained timer and return the
    /// clocked client.
    ///
    /// The caller has released Controller output and left the shared PHY
    /// domain and BTBB; the controller-time worker holds no request.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn shut_down<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        output: oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
        timer: oer_esp32s31_hal::bluetooth::ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::engine::EngineShutDown<M>,
        oer_esp32s31_hal::bluetooth::engine::EngineShutdownFailure<M>,
    > {
        self.engine.shut_down(radio, output, timer)
    }

    /// Preserve pending or faulted time ownership before any terminal extraction.
    pub fn controller_time_retirement_ready(
        &self,
    ) -> Result<(), crate::controller_time::ControllerTimeRetirementError> {
        self.controller_time.retirement_ready()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn release_controller_output(
        &mut self,
        output: oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
        clock: &dyn oer_time::Clock,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
        (
            oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError,
            oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
        ),
    > {
        self.engine
            .release_controller_output(output, crate::diagnostic_settle(clock))
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<M: ControllerMemory> TaskResources<M> {
    /// Publish the selected random Controller identity while every radio role is idle.
    ///
    /// The HCI boundary owns validation and HCI byte order. The caller must
    /// invoke this before transferring any advertising/scanning graph or
    /// publishing scheduler `RUN`; the HAL fixes the destination address slot.
    #[cfg(target_arch = "riscv32")]
    pub fn program_random_device_address_while_idle(
        &mut self,
        address: ControllerRandomAddress,
    ) -> Result<(), oer_esp32s31_hal::bluetooth::engine::EngineFault> {
        self.engine
            .program_random_device_address_while_idle(address)
    }

    /// Execute the source-127 register prefix and following complete low-power
    /// hardware component while the upper lifecycle retains initialized
    /// Controller software and an inactive route.
    ///
    /// The caller owns the matching powered scheduler/HCI epoch, has not
    /// installed source 127, and retains the returned disjoint timer owner
    /// until verified route teardown.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn initialize_modem_lp_timer_hardware(
        &mut self,
    ) -> Result<ModemLpTimerLowPowerHardwareInitializedOwner, BluetoothModemLpTimerOwnerError> {
        self.engine.initialize_modem_lp_timer_hardware()
    }

    /// Remove every scheduler hardware-list head through one finite HAL borrow.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        self.engine.clear_scheduler_hardware_list_heads()
    }

    /// Durable logical phase paired with this unique task owner.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn controller_time_phase(&self) -> ControllerTimeWorkerPhase {
        self.controller_time.phase()
    }

    /// Whether the runner must retain a durable recheck event or deadline.
    #[cfg(test)]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.controller_time.needs_recheck()
    }

    /// Publish through the scheduler's exclusive idle-task capability.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_idle_controller_time(
        &mut self,
    ) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
        let Self {
            engine,
            controller_time,
        } = self;
        if engine.is_quarantined() {
            return Err(ControllerTimeRequestError::Faulted);
        }
        let idle = match core::mem::replace(controller_time, ControllerTimeWorker::Faulted) {
            ControllerTimeWorker::Idle(idle) => idle,
            active => {
                *controller_time = active;
                return Err(ControllerTimeRequestError::OwnershipCollision);
            }
        };
        let (next, result) = idle.request(engine);
        *controller_time = next;
        result
    }

    /// Cancel only the matching logical request; a mismatch faults the worker.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        self.controller_time.cancel_owned(request)
    }

    /// Recheck one exact request with a short HAL borrow.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        let Self {
            engine,
            controller_time,
        } = self;
        controller_time.recheck_owned(request, engine)
    }

    /// Drain one abandoned request without creating a reusable sample.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        let Self {
            engine,
            controller_time,
        } = self;
        controller_time.drain_orphan(engine)
    }

    /// Execute the complete reviewed controller HAL-init component.
    ///
    /// The owning lifecycle invokes this component after clocks and before
    /// scheduler initialization. Later event/list, interrupt, PHY, BTBB and
    /// BLE stages remain separate prerequisites for a running controller.
    ///
    /// The caller does not infer controller or HCI readiness from return.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        dead_code,
        reason = "only the target lifecycle and isolated validation probes invoke this component"
    )]
    pub(crate) fn initialize_controller_hal(
        &mut self,
        config: BluetoothControllerHalInitConfig,
    ) -> Result<(), oer_esp32s31_hal::bluetooth::engine::EngineInitError> {
        self.engine.initialize_controller_hal(config)
    }

    /// The one engine holding this task's register token and complete arena.
    pub fn engine(&mut self) -> &mut oer_esp32s31_hal::bluetooth::engine::Engine<M> {
        &mut self.engine
    }

    pub fn engine_retirement_ready(&self) -> bool {
        self.engine.retirement_ready()
    }
}

/// Inactive owner of the Bluetooth controller interrupt bank.
#[must_use = "the interrupt owner must be installed or reunited"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct InterruptBankOwner {
    _registers: HalBluetoothInterruptSetupOwner,
}

#[cfg(target_arch = "riscv32")]
impl InterruptBankOwner {
    /// Prepare the controller output while retaining the affine IRQ partition.
    ///
    /// The caller owns the matching completed Controller initialization, and
    /// all three CPU routes remain inactive.
    pub(crate) fn prepare_controller_output(self) -> InterruptOutputPreparedOwner {
        self._registers.prepare_controller_output()
    }
}

#[cfg(test)]
mod tests;
