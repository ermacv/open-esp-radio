//! Hardware-only modem low-power initialization and runtime ownership.

use crate::{
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
    },
    scheduler::{SchedulerInitialized, SchedulerSoftwareConfig},
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerOwnerError,
    ModemLpTimerLowPowerHardwareInitializedOwner,
};

use oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale;

/// Powered Controller ownership after the complete modem low-power hardware
/// component and before source 127 is installed.
///
/// The disjoint timer owner stays inside this state. It is not yet in stable
/// ISR storage, no CPU route is enabled, and this state therefore exposes no
/// live timer interrupt or operational Link-Layer capability.
#[must_use = "the initialized low-power hardware retains every powered Bluetooth owner"]
pub struct ControllerLowPowerHardwareInitialized<'cells, const MODEM_TIMER_CAPACITY: usize> {
    scheduler: SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY>,
    timer_hardware: Option<ModemLpTimerLowPowerHardwareInitializedOwner>,
}

/// Lossless failure to initialize the disjoint modem low-power timer owner.
#[must_use = "the powered scheduler remains owned after a rejected transition"]
pub struct ControllerLowPowerHardwareInitializationFailure<
    'cells,
    const MODEM_TIMER_CAPACITY: usize,
> {
    scheduler: SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY>,
    error: BluetoothModemLpTimerOwnerError,
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerLowPowerHardwareInitializationFailure<'cells, MODEM_TIMER_CAPACITY>
{
    /// Exact lower ownership error.
    pub const fn error(&self) -> BluetoothModemLpTimerOwnerError {
        self.error
    }

    /// Recover the complete powered Controller owner.
    pub fn into_scheduler(self) -> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
        self.scheduler
    }
}

/// Task HAL owner and software endpoints of one epoch.
///
/// Named fields keep interrupt, task and modem-timer roles explicit. HCI is not
/// part of this hardware-only split and is joined only after stable interrupt
/// publication.
#[must_use = "all runtime endpoints belong to one powered Controller epoch"]
pub struct ControllerRuntimeEndpoints<'runtime, const MODEM_TIMER_CAPACITY: usize> {
    /// Interrupt-side scheduler publications.
    pub interrupt: ControllerInterruptRuntime<'runtime>,
    /// Task-side HAL owner and scheduler workers.
    pub task: ControllerPoweredTaskRuntime<'runtime>,
    /// Unique source-127 queue and epoch runtime.
    pub modem_timer: ControllerModemTimerRuntime<'runtime, MODEM_TIMER_CAPACITY>,
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize> SchedulerInitialized<'cells, MODEM_TIMER_CAPACITY> {
    fn try_initialize_low_power_hardware_with<TimerHardware, Error>(
        mut self,
        initialize: impl FnOnce(&mut crate::resources::TaskResources) -> Result<TimerHardware, Error>,
    ) -> Result<(Self, TimerHardware), (Self, Error)> {
        match initialize(self.task_mut()) {
            Ok(timer_hardware) => Ok((self, timer_hardware)),
            Err(error) => Err((self, error)),
        }
    }

    /// Execute the source-127 register prefix and complete modem low-power
    /// hardware initialization while the CPU route remains inactive.
    ///
    /// The safe Controller typestate discharges the lower HAL prerequisites:
    /// clocks, scheduler software and both disjoint register owners belong
    /// to this exact powered epoch, while the only route installer remains
    /// private and cannot have run yet.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the consuming Controller state proves the lower HAL prerequisites"
    )]
    pub fn initialize_modem_lp_timer_hardware(
        self,
    ) -> Result<
        ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
        ControllerLowPowerHardwareInitializationFailure<'cells, MODEM_TIMER_CAPACITY>,
    > {
        match self.try_initialize_low_power_hardware_with(|task| {
            // SAFETY: this consuming state owns the powered scheduler
            // epoch and no public transition can have installed source 127.
            unsafe { task.initialize_modem_lp_timer_hardware() }
        }) {
            Ok((scheduler, timer_hardware)) => Ok(ControllerLowPowerHardwareInitialized {
                scheduler,
                timer_hardware: Some(timer_hardware),
            }),
            Err((scheduler, error)) => {
                Err(ControllerLowPowerHardwareInitializationFailure { scheduler, error })
            }
        }
    }
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>
{
    /// Number of scheduler modem-timer slots retained by this exact epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.scheduler.modem_timer_capacity()
    }

    /// Scheduler time scale retained by this exact powered epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.scheduler.controller_time_scale()
    }

    /// Source-owned scheduler policy retained by this exact powered epoch.
    pub const fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        self.scheduler.scheduler_config()
    }

    /// Whether no scheduler software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.scheduler.runtime_is_pristine()
    }

    /// Hand the task owner and software workers to their endpoints once the
    /// interrupt bank and timer hardware were activated.
    pub fn split_runtime(self) -> ControllerRuntimeEndpoints<'cells, MODEM_TIMER_CAPACITY> {
        assert!(
            self.timer_hardware.is_none(),
            "the timer hardware is activated before the runtime splits"
        );
        let (interrupt, task, modem_timer) = self.scheduler.split_runtime();
        ControllerRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn task(&self) -> &crate::resources::TaskResources {
        self.scheduler.task()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn task_mut(&mut self) -> &mut crate::resources::TaskResources {
        self.scheduler.task_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> crate::resources::InterruptBankOwner {
        self.scheduler.take_interrupt_owner()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_timer_hardware(&mut self) -> ModemLpTimerLowPowerHardwareInitializedOwner {
        self.timer_hardware.take().expect(
            "private Controller invariant retains the low-power timer owner until activation",
        )
    }

    /// Conditional runtime-control branch observed by the exact hardware
    /// component.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.timer_hardware
            .as_ref()
            .expect("public low-power state always retains its timer owner")
            .runtime_control_observation()
    }
}

#[cfg(test)]
mod tests;
