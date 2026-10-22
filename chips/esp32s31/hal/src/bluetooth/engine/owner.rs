//! Register operations and absorbing terminal custody of the whole arena.

use super::{ControllerMemory, Engine};
use crate::bluetooth::{self, ControllerHalBorrow};

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
    ) {
        self.registers.initialize_controller_hal_transaction(config);
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
    ) {
        self.memory
            .phy()
            .initialize_registers(&mut self.registers, radio);
        let mut controller = self.registers.borrow_bluetooth_controller();
        controller.program_public_device_address(address);
        self.direction_finding_publication = Some(
            self.memory
                .direction_finding()
                .publish_disabled_baseline(&mut controller),
        );
    }

    pub fn le_1m_packet_start_calibration(&self) -> super::BlePhyLe1MPacketStartCalibration {
        self.memory.phy().le_1m_packet_start_calibration()
    }
    pub fn direction_finding_link(&self) -> super::DirectionFindingWorkspaceLink {
        self.memory.direction_finding().link()
    }

    pub const fn is_quarantined(&self) -> bool {
        self.faulted
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
    pub fn transfer_scheduler_finished_lists(
        &mut self,
    ) -> bluetooth::BluetoothSchedulerFinishedListObservation {
        self.controller().transfer_scheduler_finished_lists()
    }
    pub fn program_random_device_address_while_idle(
        &mut self,
        address: bluetooth::ControllerRandomAddress,
    ) -> Result<(), super::EngineFault> {
        if self.faulted {
            return Err(super::EngineFault);
        }
        self.controller().program_random_device_address(address);
        Ok(())
    }

    pub fn retirement_ready(&self) -> bool {
        !self.faulted && self.memory.retirement_ready()
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
            direction_finding_publication,
            faulted: _,
        } = self;
        match registers.shut_down(radio, output, timer) {
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
