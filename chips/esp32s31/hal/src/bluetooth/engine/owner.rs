//! Register operations and absorbing terminal custody of the whole arena.

use super::{ControllerMemory, Engine};
use crate::bluetooth::{self, ControllerHalBorrow};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineInitError {
    AlreadyInitialized,
    HalNotInitialized,
    Quarantined,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineShutdownError {
    NotStopped,
    Reset(bluetooth::BluetoothShutdownError),
}

/// A terminal failure retains the complete arena and every register partition.
#[must_use = "quarantined hardware and arena remain retained until chip reset"]
pub struct EngineShutdownFailure<M: ControllerMemory> {
    error: EngineShutdownError,
    _retained: ShutdownRetention<M>,
}

#[allow(dead_code, clippy::large_enum_variant)]
enum ShutdownRetention<M: ControllerMemory> {
    NotStopped {
        engine: Engine<M>,
        output: bluetooth::InterruptOutputReleasedOwner,
        timer: bluetooth::ModemLpTimerInterruptReadyOwner,
    },
    Reset {
        registers: bluetooth::BluetoothShutdownFailure,
        memory: M,
        publication: Option<bluetooth::DirectionFindingDisabledBaselineOwner>,
    },
}

impl<M: ControllerMemory> EngineShutdownFailure<M> {
    pub const fn error(&self) -> EngineShutdownError {
        self.error
    }
}
impl<M: ControllerMemory> core::fmt::Debug for EngineShutdownFailure<M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineShutdownFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// CPU ownership returns only after the matching controller reset succeeds.
pub struct EngineShutDown<M: ControllerMemory> {
    pub clocked: bluetooth::ClockedOwner,
    pub arena: crate::dma_static::Placed<M::Storage>,
    pub reset: bluetooth::BluetoothControllerReset,
}

impl<M: ControllerMemory> Engine<M> {
    /// Disable the PHY ETM route during a no-CTE DTM session. The caller
    /// retains the token until its stopped session restores the route.
    pub fn disable_dtm_etm_route(&mut self) -> bluetooth::BlePhyEtmRouteDisabled {
        self.registers
            .borrow_bluetooth_controller()
            .disable_ble_phy_etm_route()
    }

    /// Restore the same session's route after its physical scheduler stopped.
    pub fn restore_dtm_etm_route(&mut self, route: bluetooth::BlePhyEtmRouteDisabled) {
        self.registers
            .borrow_bluetooth_controller()
            .restore_ble_phy_etm_route(route);
    }

    /// Non-repeating identity of this engine installation, for lifecycle replies.
    pub const fn installation(&self) -> u32 {
        self.epoch.0
    }

    /// Bind the unique task request endpoint to this engine's installation.
    /// The returned P3 half lives beside the stable interrupt register owner.
    pub fn install_scheduler_interrupts(
        &mut self,
        channel: &'static super::scheduler::interrupt::SchedulerInterruptChannel,
    ) -> Result<
        super::scheduler::interrupt::SchedulerInterruptHalf,
        super::scheduler::interrupt::SchedulerInterruptError,
    > {
        use super::scheduler::interrupt::SchedulerInterruptError;
        if self.is_quarantined() {
            return Err(SchedulerInterruptError::Quarantined);
        }
        if self.scheduler_interrupts.is_some() {
            return Err(SchedulerInterruptError::Occupied);
        }
        let (task, interrupts) = channel.install(self.epoch.0)?;
        self.scheduler_interrupts = Some(task);
        Ok(interrupts)
    }

    /// After stop and route removal, reunite both endpoints before reset.
    /// Pending or abandoned work keeps the channel and whole arena reserved.
    pub fn retire_scheduler_interrupts(
        &mut self,
        interrupts: super::scheduler::interrupt::SchedulerInterruptHalf,
    ) -> Result<
        (),
        (
            super::scheduler::interrupt::SchedulerInterruptError,
            super::scheduler::interrupt::SchedulerInterruptHalf,
        ),
    > {
        use super::scheduler::interrupt::SchedulerInterruptError;
        if self.is_quarantined() {
            return Err((SchedulerInterruptError::Quarantined, interrupts));
        }
        if !self.memory.scheduler_stopped() {
            return Err((SchedulerInterruptError::NotStopped, interrupts));
        }
        let Some(task) = self.scheduler_interrupts.take() else {
            return Err((SchedulerInterruptError::NotInstalled, interrupts));
        };
        match task.retire(interrupts) {
            Ok(()) => Ok(()),
            Err((error, task, interrupts)) => {
                self.scheduler_interrupts = Some(task);
                Err((error, interrupts))
            }
        }
    }

    /// Borrow register authority for shared PHY membership, without extraction.
    pub const fn task_owner(&self) -> &bluetooth::TaskOwner {
        &self.registers
    }

    /// One finite controller operation; it cannot detach the register token.
    pub(crate) fn controller(&mut self) -> bluetooth::ControllerHal<'_> {
        self.registers.borrow_bluetooth_controller()
    }

    pub fn initialize_controller_hal(
        &mut self,
        config: bluetooth::BluetoothControllerHalInitConfig,
    ) -> Result<(), EngineInitError> {
        self.initialize_controller_hal_with(config, |task, config| {
            task.initialize_controller_hal_transaction(config)
        })
    }

    fn initialize_controller_hal_with(
        &mut self,
        config: bluetooth::BluetoothControllerHalInitConfig,
        initialize: impl FnOnce(&mut bluetooth::TaskOwner, bluetooth::BluetoothControllerHalInitConfig),
    ) -> Result<(), EngineInitError> {
        if self.is_quarantined() {
            return Err(EngineInitError::Quarantined);
        }
        if self.initialized {
            return Err(EngineInitError::AlreadyInitialized);
        }
        self.initialized = true;
        initialize(&mut self.registers, config);
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn initialize_hal_for_host_test(
        &mut self,
        config: bluetooth::BluetoothControllerHalInitConfig,
    ) -> Result<(), EngineInitError> {
        self.initialize_controller_hal_with(config, |_, _| {})
    }

    pub fn initialize_modem_lp_timer_hardware(
        &mut self,
    ) -> Result<
        bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner,
        bluetooth::BluetoothModemLpTimerOwnerError,
    > {
        let prepared = self.registers.prepare_modem_lp_timer_registers()?;
        Ok(prepared.initialize_low_power_hardware(&mut self.registers))
    }

    #[cfg(target_arch = "riscv32")]
    pub fn initialize_ble_phy<B: crate::power::PlatformClockBackend, T>(
        &mut self,
        radio: &mut crate::domain::RadioDomain<B, T>,
        address: bluetooth::ControllerPublicAddress,
    ) -> Result<(), EngineInitError> {
        self.initialize_phy_with(|registers, memory| {
            memory.phy().initialize_registers(registers, radio);
            let mut controller = registers.borrow_bluetooth_controller();
            controller.program_public_device_address(address);
            memory
                .direction_finding()
                .publish_disabled_baseline(&mut controller)
        })
    }

    fn initialize_phy_with(
        &mut self,
        initialize: impl FnOnce(
            &mut bluetooth::TaskOwner,
            &M,
        ) -> bluetooth::DirectionFindingDisabledBaselineOwner,
    ) -> Result<(), EngineInitError> {
        if self.is_quarantined() {
            return Err(EngineInitError::Quarantined);
        }
        if !self.initialized {
            return Err(EngineInitError::HalNotInitialized);
        }
        if self.phy_initialized {
            return Err(EngineInitError::AlreadyInitialized);
        }
        self.phy_initialized = true;
        self.direction_finding_publication = Some(initialize(&mut self.registers, &self.memory));
        Ok(())
    }

    pub fn le_1m_packet_start_calibration(&self) -> super::BlePhyLe1MPacketStartCalibration {
        self.memory.phy().le_1m_packet_start_calibration()
    }
    pub(crate) fn direction_finding_link(&self) -> super::DirectionFindingWorkspaceLink {
        self.memory.direction_finding().link()
    }

    pub const fn is_quarantined(&self) -> bool {
        self.faulted
            || match &self.scheduler_interrupts {
                Some(task) => !task.is_quiet(),
                None => false,
            }
    }

    pub fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), bluetooth::BluetoothControllerTimeLatchBeginError> {
        self.controller().begin_controller_time_latch()
    }
    pub fn step_controller_time_latch(
        &mut self,
    ) -> Result<
        bluetooth::BluetoothControllerTimeLatchStep,
        bluetooth::BluetoothControllerTimeLatchStepError,
    > {
        self.controller().step_controller_time_latch()
    }
    pub fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> bluetooth::BluetoothSchedulerHardwareListsCleared {
        self.controller().clear_scheduler_hardware_list_heads()
    }
    pub fn transfer_scheduler_finished_lists(&mut self) -> super::EngineFinishedListObservation {
        let observed = self.controller().transfer_scheduler_finished_lists();
        super::EngineFinishedListObservation::new(observed, self.epoch)
    }
    pub fn program_random_device_address_while_idle(
        &mut self,
        address: bluetooth::ControllerRandomAddress,
    ) -> Result<(), super::EngineFault> {
        if self.is_quarantined() {
            return Err(super::EngineFault);
        }
        self.controller().program_random_device_address(address);
        Ok(())
    }

    pub fn retirement_ready(&self) -> bool {
        !self.faulted && self.scheduler_interrupts.is_none() && self.memory.retirement_ready()
    }

    /// The stop receipt remains inside the owning engine while a task drains
    /// its final finished lists.
    pub fn scheduler_is_stopped(&self) -> bool {
        !self.is_quarantined() && self.memory.scheduler_stopped()
    }

    pub fn release_controller_output(
        &mut self,
        output: bluetooth::InterruptOutputAfterRoutesOwner,
        settle: crate::settle::Settle<'_, dyn oer_time::Clock + '_>,
    ) -> Result<
        bluetooth::InterruptOutputReleasedOwner,
        (
            bluetooth::BluetoothControllerOutputReleaseError,
            bluetooth::InterruptOutputAfterRoutesOwner,
        ),
    > {
        output.try_release_idle_controller_output(&mut self.registers, settle)
    }

    #[allow(clippy::result_large_err)]
    pub fn shut_down<B: crate::power::PlatformClockBackend, T>(
        self,
        radio: &mut crate::domain::RadioDomain<B, T>,
        output: bluetooth::InterruptOutputReleasedOwner,
        timer: bluetooth::ModemLpTimerInterruptReadyOwner,
    ) -> Result<EngineShutDown<M>, EngineShutdownFailure<M>> {
        self.shut_down_with(output, timer, |registers, output, timer| {
            registers.shut_down(radio, output, timer)
        })
    }

    #[allow(clippy::result_large_err)]
    fn shut_down_with(
        self,
        output: bluetooth::InterruptOutputReleasedOwner,
        timer: bluetooth::ModemLpTimerInterruptReadyOwner,
        reset_registers: impl FnOnce(
            bluetooth::TaskOwner,
            bluetooth::InterruptOutputReleasedOwner,
            bluetooth::ModemLpTimerInterruptReadyOwner,
        ) -> Result<
            (bluetooth::ClockedOwner, bluetooth::BluetoothControllerReset),
            bluetooth::BluetoothShutdownFailure,
        >,
    ) -> Result<EngineShutDown<M>, EngineShutdownFailure<M>> {
        if !self.retirement_ready() {
            return Err(EngineShutdownFailure {
                error: EngineShutdownError::NotStopped,
                _retained: ShutdownRetention::NotStopped {
                    engine: self,
                    output,
                    timer,
                },
            });
        }
        let Self {
            registers,
            memory,
            epoch: _,
            scheduler_interrupts: _,
            direction_finding_publication,
            faulted: _,
            initialized: _,
            phy_initialized: _,
        } = self;
        match reset_registers(registers, output, timer) {
            Ok((clocked, reset)) => Ok(EngineShutDown {
                clocked,
                arena: memory.reclaim(&reset),
                reset,
            }),
            Err(registers) => Err(EngineShutdownFailure {
                error: EngineShutdownError::Reset(registers.error()),
                _retained: ShutdownRetention::Reset {
                    registers,
                    memory,
                    publication: direction_finding_publication,
                },
            }),
        }
    }
}

#[cfg(test)]
mod tests;
