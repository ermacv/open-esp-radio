//! Task and interrupt owners of one powered Controller epoch.
//!
//! The epoch starts from the HAL's clocked Bluetooth client, which proves
//! common radio power, the Bluetooth module clocks, the released controller
//! resets and the low-power timer clock. `separate_interrupt_owner` splits
//! it into the task-side [`TaskResources`] and the inactive
//! `InterruptBankOwner`; `TaskResources::shut_down` resets the stopped
//! Controller and returns the clocked client.

#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListsCleared;

#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig;
#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use oer_esp32s31_hal::bluetooth::ControllerHalBorrow;
#[cfg(test)]
use oer_esp32s31_hal::bluetooth::TaskOwnerReuniteFailure;

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
use oer_esp32s31_hal::bluetooth::{
    ClockedOwner, InterruptSetupOwner as HalBluetoothInterruptSetupOwner,
    TaskOwner as HalBluetoothTaskOwner,
};
#[cfg(target_arch = "riscv32")]
use {
    oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerOwnerError,
    oer_esp32s31_hal::bluetooth::ControllerPublicAddress,
    oer_esp32s31_hal::bluetooth::ControllerRandomAddress,
    oer_esp32s31_hal::bluetooth::DirectionFindingDisabledBaselineOwner,
    oer_esp32s31_hal::bluetooth::InterruptOutputPreparedOwner,
    oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner,
    oer_esp32s31_hal::shared_radio::SharedRadioLease,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
    oer_esp32s31_hal::types::BluetoothPhyRegisterInitInputs,
};

#[cfg(any(target_arch = "riscv32", test))]
use crate::controller_time::ControllerTimeWorker;
#[cfg(test)]
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
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) fn separate_interrupt_owner(
    clocked: ClockedOwner,
) -> (TaskResources, InterruptBankOwner) {
    let (task, interrupts) = clocked.separate_interrupt_owner();
    (
        TaskResources {
            registers: task,
            #[cfg(any(target_arch = "riscv32", test))]
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
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub struct TaskResources {
    registers: HalBluetoothTaskOwner,
    // Host validation images execute finite register probes without a time runner.
    #[cfg(any(target_arch = "riscv32", test))]
    controller_time: ControllerTimeWorker,
}

#[cfg(any(target_arch = "riscv32", test))]
impl TaskResources {
    /// The HAL task owner, which proves the Bluetooth module clocks to the
    /// shared PHY domain and BTBB.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn hal(&self) -> &HalBluetoothTaskOwner {
        &self.registers
    }

    /// Reset the stopped Controller, reunite the drained timer and return the
    /// clocked client.
    ///
    /// The caller has released Controller output and left the shared PHY
    /// domain and BTBB; the controller-time worker holds no request.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn shut_down<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
        output: oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
        timer: oer_esp32s31_hal::bluetooth::ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        (
            ClockedOwner,
            oer_esp32s31_hal::bluetooth::BluetoothControllerReset,
        ),
        oer_esp32s31_hal::bluetooth::BluetoothShutdownFailure,
    > {
        self.registers.shut_down(lease, output, timer)
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
    ) -> Result<
        oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
        (
            oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError,
            oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
        ),
    > {
        output.try_release_idle_controller_output(
            &mut self.registers,
            crate::scheduler::DIAGNOSTIC_READ_BUDGET,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
impl TaskResources {
    /// Publish the selected random Controller identity while every radio role is idle.
    ///
    /// The HCI boundary owns validation and HCI byte order. The caller must
    /// invoke this before transferring any advertising/scanning graph or
    /// publishing scheduler `RUN`; the HAL fixes the destination address slot.
    #[cfg(target_arch = "riscv32")]
    pub fn program_random_device_address_while_idle(&mut self, address: ControllerRandomAddress) {
        self.registers
            .borrow_bluetooth_controller()
            .program_random_device_address(address);
    }

    /// Execute the source-127 register prefix and following complete low-power
    /// hardware component while the upper lifecycle retains initialized
    /// Controller software and an inactive route.
    ///
    /// # Safety
    ///
    /// The caller must own the matching powered scheduler/HCI epoch, must not
    /// have installed source 127, and must retain the returned disjoint timer
    /// owner until verified route teardown.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the upper lifecycle proves the powered software and inactive-route prerequisites"
    )]
    pub(crate) unsafe fn initialize_modem_lp_timer_hardware(
        &mut self,
    ) -> Result<ModemLpTimerLowPowerHardwareInitializedOwner, BluetoothModemLpTimerOwnerError> {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the lower transaction's prerequisites.
        let prepared = unsafe { self.registers.prepare_modem_lp_timer_registers()? };
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the lower transaction's prerequisites.
        Ok(unsafe { prepared.initialize_low_power_hardware(&mut self.registers) })
    }

    /// Remove every scheduler hardware-list head through one finite HAL borrow.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    pub(crate) fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        self.registers
            .borrow_bluetooth_controller()
            .clear_scheduler_hardware_list_heads()
    }

    /// The task-side HAL for one finite operation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn controller(&mut self) -> oer_esp32s31_hal::bluetooth::ControllerHal<'_> {
        self.registers.borrow_bluetooth_controller()
    }

    /// Durable logical phase paired with this unique task owner.
    #[cfg(test)]
    pub(crate) const fn controller_time_phase(&self) -> ControllerTimeWorkerPhase {
        self.controller_time.phase()
    }

    /// Whether the runner must retain a durable recheck event or deadline.
    #[cfg(test)]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.controller_time.needs_recheck()
    }

    /// Publish one request while retaining worker and PAC ownership together.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
        let Self {
            registers,
            controller_time,
        } = self;
        let mut controller = registers.borrow_bluetooth_controller();
        controller_time.request(&mut controller)
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
            registers,
            controller_time,
        } = self;
        let mut controller = registers.borrow_bluetooth_controller();
        controller_time.recheck_owned(request, &mut controller)
    }

    /// Drain one abandoned request without creating a reusable sample.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        let Self {
            registers,
            controller_time,
        } = self;
        let mut controller = registers.borrow_bluetooth_controller();
        controller_time.drain_orphan(&mut controller)
    }

    /// Execute the complete reviewed controller HAL-init component.
    ///
    /// The owning lifecycle invokes this component after clocks and before
    /// scheduler initialization. Later event/list, interrupt, PHY, BTBB and
    /// BLE stages remain separate prerequisites for a running controller.
    ///
    /// # Safety
    ///
    /// The caller must retain every prerequisite documented by the PAC
    /// transaction and must not infer controller or HCI readiness from return.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    #[allow(
        unsafe_code,
        reason = "the unsafe bridge retains the controller HAL-init clock and IRQ prerequisites"
    )]
    #[allow(
        dead_code,
        reason = "only the target lifecycle and isolated validation probes invoke this component"
    )]
    pub(crate) unsafe fn initialize_controller_hal(
        &mut self,
        config: BluetoothControllerHalInitConfig,
    ) {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the lower transaction's prerequisites.
        unsafe {
            self.registers.initialize_controller_hal_transaction(config);
        }
    }

    /// Execute the complete BLE base-stack task-enable hardware transaction
    /// for a lifecycle that retains both address-bound storage objects.
    ///
    /// # Safety
    ///
    /// The caller must retain the Bluetooth membership of the shared PHY
    /// domain and BTBB, the inactive interrupt bank, and the storage
    /// represented by `inputs` until all controller consumers are stopped by
    /// a verified transition.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the upper typestate retains the complete PAC lifecycle and storage prerequisites"
    )]
    pub(crate) unsafe fn enable_ble_base_stack_hardware<T>(
        &mut self,
        lease: &mut SharedRadioLease<'_, T>,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the lower transaction's prerequisites.
        unsafe {
            self.registers.enable_ble_base_stack_hardware(lease, inputs);
        }
    }

    /// Publish this epoch's public Controller identity after BLE PHY init.
    ///
    /// The caller retains the sole powered task owner and invokes this before
    /// controller IRQ output, runtime-timer start or any radio consumer becomes
    /// reachable. The HAL fixes the public slot and owns Controller byte order.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn program_public_device_address(&mut self, address: ControllerPublicAddress) {
        self.registers
            .borrow_bluetooth_controller()
            .program_public_device_address(address);
    }

    /// Publish the controller-global disabled-CTE descriptor baseline.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned workspace and this
    /// powered Controller epoch until a future reviewed retirement transition.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the upper BLE-PHY lifecycle retains the workspace and powered task owner"
    )]
    pub(crate) unsafe fn prepare_direction_finding_disabled_baseline(
        &mut self,
        descriptor: BluetoothControllerSramAddress,
    ) -> DirectionFindingDisabledBaselineOwner {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the lower transaction's prerequisites.
        unsafe {
            self.registers
                .borrow_bluetooth_controller()
                .prepare_direction_finding_disabled_baseline(descriptor)
        }
    }

    /// Reunite a quiescent task owner with its inactive interrupt owner.
    #[cfg(test)]
    pub(crate) fn reunite(
        self,
        interrupts: InterruptBankOwner,
    ) -> Result<ClockedOwner, TaskOwnerReuniteFailure> {
        assert!(
            self.controller_time.is_reunitable(),
            "controller-time fault or transaction prevents reunion"
        );
        self.registers.into_clocked(interrupts._registers)
    }
}

/// Inactive owner of the Bluetooth controller interrupt bank.
#[must_use = "the interrupt owner must be installed or reunited"]
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) struct InterruptBankOwner {
    _registers: HalBluetoothInterruptSetupOwner,
}

#[cfg(target_arch = "riscv32")]
impl InterruptBankOwner {
    /// Prepare the controller output while retaining the affine IRQ partition.
    ///
    /// # Safety
    ///
    /// The caller must own the matching completed Controller initialization
    /// and prove that all three CPU routes remain inactive.
    #[allow(
        unsafe_code,
        reason = "the upper Controller typestate discharges the HAL interrupt prerequisites"
    )]
    pub(crate) unsafe fn prepare_controller_output(self) -> InterruptOutputPreparedOwner {
        // SAFETY: the caller retains the complete matching Controller epoch
        // and the only route installers are still inaccessible.
        unsafe { self._registers.prepare_controller_output() }
    }
}

#[cfg(test)]
mod tests;
