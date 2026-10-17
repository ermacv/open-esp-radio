//! ESP32-S31 Bluetooth controller partition as a client of the shared radio.
//!
//! The chain starts from the [`BluetoothPartition`] of a concurrent split and
//! holds only the controller, its modem low-power timer and its interrupt
//! bank. Common radio power, the module clocks, the controller domain resets
//! and the low-power timer clock live in the shared radio registers, so each
//! of those transitions borrows the [`RadioDomain`] for its
//! duration and never retains it.
//!
//! [`ColdOwner`] enters common power as [`PoweredOwner`], which enables the
//! Bluetooth module clocks as [`ClockedOwner`]. That owner separates into the
//! task-side [`TaskOwner`] and the inactive [`InterruptSetupOwner`]; the
//! lifecycle returns to it by pristine reunion or by the verified Controller
//! shutdown. Lower layers receive only finite borrows and named operations;
//! they cannot recover, move or duplicate the underlying register partition.

#![deny(unsafe_code)]

use core::fmt;

use oer_esp32s31_pac::{
    BluetoothDirectionFindingDisabledBaselinePrepared, BluetoothInterruptRegisters,
    BluetoothInterruptSetup as PacBluetoothInterruptSetup, BluetoothModemLpTimerRegisters,
    BluetoothTaskRegisters,
};

use crate::{
    domain::{
        BluetoothClient, BtbbAcquired, BtbbError, CoexAbsent, CommonPower, CommonPowerExitFailure,
        CommonRadioPowerError, LowPowerClockError, ModemClockError, PlatformClockProvider,
        RadioDomain,
    },
    root::BluetoothPartition,
};

mod controller_time;
mod diagnostic;
mod scheduler_stop;
mod shutdown;

use controller_time::ControllerTimeLatch;
pub use controller_time::{
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchRequest,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
};
pub use diagnostic::{BluetoothDiagnosticReadBudget, BluetoothDiagnosticUnsettled};
pub use scheduler_stop::{BluetoothSchedulerStop, BluetoothSchedulerStopStep};
pub use shutdown::{BluetoothControllerReset, BluetoothShutdownError, BluetoothShutdownFailure};

pub use oer_esp32s31_pac::{
    BlePhyEtmRouteDisabled, BluetoothControllerLatchedTime, BluetoothControllerOutputReleaseError,
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothDeviceTableEntryCount, BluetoothDeviceTablePublished,
    BluetoothLowPowerClockObservation, BluetoothLowPowerRuntimeControlObservation,
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerCounterObservation,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerRegisterObservation,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothNrtInterruptAcknowledged, BluetoothPrimaryFaultSources,
    BluetoothPrimaryInterruptEpoch, BluetoothScanStartPublished, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerCancellationDisposition, BluetoothSchedulerCancellationIndexed,
    BluetoothSchedulerCancellationReleased, BluetoothSchedulerCancellationRequested,
    BluetoothSchedulerCancellationSourceAcknowledged,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListHeadError,
    BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation, BluetoothSchedulerReferenceCleared,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerRunEventPublished,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerSkipCleared,
    BluetoothSchedulerSkipDisposition, BluetoothSchedulerSkipPublished,
    BluetoothSchedulerSkipRequest, BluetoothSchedulerSkipResult,
    BluetoothSchedulerSoftwareListRemovalIdle, BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
    BluetoothSchedulerStopped, BluetoothSchedulerStoppedHeadRetirement,
    BluetoothSchedulerStoppedItem, BluetoothSchedulerWorkObservation,
    ModemSysconBluetoothObservation, PlatformClockPowerObservation, SharedModemClockObservation,
};

/// The Bluetooth partition split into its task registers, modem low-power
/// timer and inactive interrupt bank.
struct Partition {
    task: BluetoothTaskRegisters,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: PacBluetoothInterruptSetup,
}

/// Unpowered Bluetooth client holding its partition.
///
/// Construction splits the partition without touching MMIO. It proves
/// neither power nor clocks, common-PHY, BTBB, IRQ or Controller readiness.
#[must_use = "the cold Bluetooth client retains its radio partition"]
pub struct ColdOwner {
    partition: Partition,
}

impl ColdOwner {
    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: BluetoothPartition) -> Self {
        let (controller, modem_lp_timer, interrupts) = partition.into_parts();
        Self {
            partition: Partition {
                task: BluetoothTaskRegisters::new(controller),
                modem_lp_timer,
                interrupts,
            },
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> BluetoothPartition {
        let Partition {
            task,
            modem_lp_timer,
            interrupts,
        } = self.partition;
        BluetoothPartition::from_parts(task.into_partition(), modem_lp_timer, interrupts)
    }

    /// Enter common radio power as the Bluetooth client.
    ///
    /// Only the first client runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power or the first client's sequence failed a
    /// read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<T>(
        self,
        radio: &mut RadioDomain<T>,
        clocks: &impl PlatformClockProvider,
    ) -> Result<(PoweredOwner, CommonPower<BluetoothClient>), PowerTransitionFailure<Self>> {
        match radio.enter_common_power(&self.partition.task, clocks) {
            Ok(power) => Ok((
                PoweredOwner {
                    partition: self.partition,
                },
                power,
            )),
            Err(error) => Err(PowerTransitionFailure { owner: self, error }),
        }
    }
}

/// Bluetooth client inside common radio power.
///
/// It proves only membership in the common modem/PHY power; the module
/// clocks, the controller resets and the low-power timer clock are the next
/// stage.
#[must_use = "the powered Bluetooth client must leave common power"]
pub struct PoweredOwner {
    partition: Partition,
}

impl PoweredOwner {
    /// Leave common radio power, outside coexistence (`coex`); the last
    /// client restores the cold baseline.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client stays entered and the
    /// unchanged owner and proof are returned for a retry.
    pub fn power_down<T>(
        self,
        radio: &mut RadioDomain<T>,
        power: CommonPower<BluetoothClient>,
        coex: &CoexAbsent<BluetoothClient>,
    ) -> Result<ColdOwner, PowerTransitionFailure<(Self, CommonPower<BluetoothClient>)>> {
        match radio.exit_common_power(&self.partition.task, power, coex) {
            Ok(()) => Ok(ColdOwner {
                partition: self.partition,
            }),
            Err(CommonPowerExitFailure { power, error }) => Err(PowerTransitionFailure {
                owner: (self, power),
                error,
            }),
        }
    }

    /// Enable the Bluetooth module clocks, reset the controller domains and
    /// select the low-power timer clock.
    ///
    /// This is the reviewed S31 Controller clock prerequisite:
    /// `modem_clock_module_enable(PERIPH_BT_MODULE)` through the shared
    /// planner, `modem_clock_module_mac_reset(PERIPH_BT_MODULE)`, and the
    /// main-crystal low-power timer clock at the S31 Bluetooth divider. The
    /// resulting clock sets, resets and low-power clock must read back before
    /// the owner advances.
    ///
    /// # Errors
    ///
    /// The planner or low-power clock selection rejected the request, or a
    /// read-back failed; every step already taken is rolled back and the
    /// owner is returned. After [`ModemClockError::Poisoned`] no further modem
    /// clock change succeeds until reset.
    pub fn enable_clocks<T>(
        self,
        radio: &mut RadioDomain<T>,
        platform: &impl PlatformClockProvider,
    ) -> Result<ClockedOwner, ClockTransitionFailure<Self>> {
        let task = &self.partition.task;
        if let Err(error) = radio.enable_modem_clocks(task, platform) {
            return Err(ClockTransitionFailure {
                owner: self,
                error: BluetoothClockError::ModemClocks(error),
            });
        }
        let mut partition = self.partition;
        partition
            .task
            .reset_controller_domains(radio.registers_mut());
        let task = &partition.task;
        if let Err(error) = radio.select_bluetooth_low_power_clock(task, platform) {
            let _ = radio.disable_modem_clocks(task);
            return Err(ClockTransitionFailure {
                owner: Self { partition },
                error: BluetoothClockError::LowPowerClock(error),
            });
        }
        let shared = radio.registers_mut();
        let clocks = task.controller_clock_observation(shared);
        let (_, low_power) = task.bluetooth_shared_clock_observation(shared);
        if let Some(checkpoint) = clock_checkpoint(clocks, low_power) {
            let _ = radio.deselect_bluetooth_low_power_clock(task);
            let _ = radio.disable_modem_clocks(task);
            return Err(ClockTransitionFailure {
                owner: Self { partition },
                error: BluetoothClockError::Readback(checkpoint),
            });
        }
        Ok(ClockedOwner { partition })
    }
}

/// First Controller clock read-back that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothClockCheckpoint {
    /// The controller dependency clock set did not read back enabled.
    ControllerClocks,
    /// The APB dependency clock set did not read back enabled.
    ApbClocks,
    /// At least one controller domain reset remained asserted.
    ControllerReset,
    /// The low-power timer clock was not sourced from the main crystal.
    LowPowerClockSource,
    /// The low-power timer divider did not match the S31 Bluetooth profile.
    LowPowerClockDivider,
}

/// Why the Bluetooth clock stage cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothClockError {
    /// The shared modem clock planner rejected or poisoned the request.
    ModemClocks(ModemClockError),
    /// The low-power timer clock selection was already in the other state.
    LowPowerClock(LowPowerClockError),
    /// A clock or reset did not read back.
    Readback(BluetoothClockCheckpoint),
}

fn clock_checkpoint(
    clocks: ModemSysconBluetoothObservation,
    low_power: BluetoothLowPowerClockObservation,
) -> Option<BluetoothClockCheckpoint> {
    if !clocks.controller_clocks_enabled {
        Some(BluetoothClockCheckpoint::ControllerClocks)
    } else if !clocks.apb_clocks_enabled {
        Some(BluetoothClockCheckpoint::ApbClocks)
    } else if !clocks.controller_resets_released {
        Some(BluetoothClockCheckpoint::ControllerReset)
    } else if !low_power.exclusive_main_xtal_selected {
        Some(BluetoothClockCheckpoint::LowPowerClockSource)
    } else if !low_power.bluetooth_divider_configured {
        Some(BluetoothClockCheckpoint::LowPowerClockDivider)
    } else {
        None
    }
}

/// Bluetooth client whose module clocks, released controller resets and
/// low-power timer clock are in effect.
///
/// It separates into the task and interrupt owners that initialize and run
/// the Controller, and is where the lifecycle returns before the clocks are
/// released.
#[must_use = "the clocked Bluetooth client must release its clocks"]
pub struct ClockedOwner {
    partition: Partition,
}

impl ClockedOwner {
    /// Deselect the low-power timer clock and disable the Bluetooth module
    /// clocks; only dependencies no other module holds are disabled.
    ///
    /// # Errors
    ///
    /// The planner rejected or poisoned the release; the owner is returned
    /// with the low-power clock already deselected, and a retry continues
    /// with the module clocks.
    pub fn disable_clocks<T>(
        self,
        radio: &mut RadioDomain<T>,
    ) -> Result<PoweredOwner, ClockTransitionFailure<Self>> {
        let task = &self.partition.task;
        // A retry after a planner failure finds the clock already deselected.
        if radio.bluetooth_low_power_clock_selected()
            && let Err(error) = radio.deselect_bluetooth_low_power_clock(task)
        {
            return Err(ClockTransitionFailure {
                owner: self,
                error: BluetoothClockError::LowPowerClock(error),
            });
        }
        match radio.disable_modem_clocks(task) {
            Ok(()) => Ok(PoweredOwner {
                partition: self.partition,
            }),
            Err(error) => Err(ClockTransitionFailure {
                owner: self,
                error: BluetoothClockError::ModemClocks(error),
            }),
        }
    }

    /// Split ordinary task ownership from the inactive controller IRQ bank.
    ///
    /// This conversion performs no MMIO and does not claim that the hardware
    /// interrupt route has been configured or enabled.
    pub fn separate_interrupt_owner(self) -> (TaskOwner, InterruptSetupOwner) {
        let Partition {
            task,
            modem_lp_timer,
            interrupts,
        } = self.partition;
        (
            TaskOwner {
                registers: task,
                modem_lp_timer: Some(modem_lp_timer),
                time_latch: ControllerTimeLatch::new(),
                reunitable: true,
            },
            InterruptSetupOwner {
                registers: interrupts,
                reunitable: true,
            },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ClockedOwner {
    /// Assume the clock stage for a host test, without any MMIO.
    #[doc(hidden)]
    pub fn for_host_test(partition: BluetoothPartition) -> Self {
        Self {
            partition: ColdOwner::from_partition(partition).partition,
        }
    }

    /// Leave the assumed clock stage without any MMIO.
    #[doc(hidden)]
    pub fn into_partition_for_host_test(self) -> BluetoothPartition {
        ColdOwner {
            partition: self.partition,
        }
        .into_partition()
    }
}

/// Failed common-power transition retaining the unchanged owner.
#[must_use = "a failed Bluetooth power transition still owns the partition"]
pub struct PowerTransitionFailure<Owner> {
    owner: Owner,
    error: CommonRadioPowerError,
}

impl<Owner> PowerTransitionFailure<Owner> {
    pub const fn error(&self) -> CommonRadioPowerError {
        self.error
    }

    /// Recover the owner for a retry.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for PowerTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PowerTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed clock-stage transition retaining the owner.
#[must_use = "a failed Bluetooth clock transition still owns the partition"]
pub struct ClockTransitionFailure<Owner> {
    owner: Owner,
    error: BluetoothClockError,
}

impl<Owner> ClockTransitionFailure<Owner> {
    pub const fn error(&self) -> BluetoothClockError {
        self.error
    }

    /// Recover the owner.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for ClockTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClockTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Opaque HAL owner for ordinary Bluetooth task-side controller registers.
///
/// It exists only between [`ClockedOwner::separate_interrupt_owner`] and the
/// return to [`ClockedOwner`], so it proves the Bluetooth module clocks.
#[must_use = "the Bluetooth task owner must return to the clocked owner"]
pub struct TaskOwner {
    registers: BluetoothTaskRegisters,
    modem_lp_timer: Option<BluetoothModemLpTimerRegisters>,
    time_latch: ControllerTimeLatch,
    reunitable: bool,
}

impl TaskOwner {
    /// Reset the stopped Controller, reunite the drained timer and return to
    /// the clocked owner with the proof of the reset.
    ///
    /// The caller has released Controller output, drained the source-127
    /// owner and left the shared PHY domain and BTBB baseband. The Controller
    /// retains every published memory owner until this operation completes.
    #[doc(hidden)]
    pub fn shut_down<T>(
        self,
        radio: &mut RadioDomain<T>,
        output: InterruptOutputReleasedOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(ClockedOwner, BluetoothControllerReset), BluetoothShutdownFailure> {
        shutdown::shut_down(self, radio, output.registers, timer.timer)
    }

    /// Reunite a quiescent, untouched task with the exact inactive interrupt
    /// partition.
    pub fn into_clocked(
        self,
        interrupts: InterruptSetupOwner,
    ) -> Result<ClockedOwner, TaskOwnerReuniteFailure> {
        if !self.reunitable {
            return Err(TaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: TaskOwnerReuniteError::HardwareLifecycleNotRestored,
            });
        }
        if !interrupts.reunitable {
            return Err(TaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: TaskOwnerReuniteError::InterruptLifecycleNotRestored,
            });
        }
        let error = if self.time_latch.in_flight() {
            Some(TaskOwnerReuniteError::ControllerTimeLatchInFlight)
        } else if self.modem_lp_timer.is_none() {
            Some(TaskOwnerReuniteError::ModemLpTimerOwnerSeparated)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(TaskOwnerReuniteFailure {
                task: TaskOwner {
                    reunitable: false,
                    ..self
                },
                interrupts,
                error,
            });
        }
        let TaskOwner {
            registers,
            modem_lp_timer,
            time_latch: _,
            reunitable: _,
        } = self;
        Ok(ClockedOwner {
            partition: Partition {
                task: registers,
                modem_lp_timer: modem_lp_timer
                    .expect("an unseparated task owner retains its modem LP-timer partition"),
                interrupts: interrupts.registers,
            },
        })
    }

    /// Take the shared BTBB baseband as the Bluetooth client.
    ///
    /// This owner holds the Bluetooth module clocks, which include the BTBB
    /// clocks. Only the first holder runs `bt_bb_v2_init_cmplx(1)`.
    ///
    /// # Errors
    ///
    /// As [`RadioDomain::btbb_acquire`].
    ///
    /// # Safety
    ///
    /// The shared PHY registration must be complete and `gain_parameter` must
    /// be the byte at offset `0x120` of that registration's `phy_param`
    /// state.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature carries the domain's common-PHY prerequisite"
    )]
    pub unsafe fn acquire_btbb<T>(
        &self,
        radio: &mut RadioDomain<T>,
        gain_parameter: u8,
    ) -> Result<BtbbAcquired, BtbbError> {
        // SAFETY: this owner holds the Bluetooth module clocks; the caller
        // upholds the registration and gain provenance.
        unsafe { radio.btbb_acquire(&self.registers, gain_parameter) }
    }

    /// Leave the shared BTBB baseband.
    ///
    /// # Errors
    ///
    /// Bluetooth does not hold BTBB.
    pub fn release_btbb<T>(&self, radio: &mut RadioDomain<T>) -> Result<(), BtbbError> {
        radio.btbb_release(&self.registers)
    }

    /// Apply the exact modem low-power timer register prefix before source 127
    /// is installed.
    ///
    /// This extracts only the generated timer partition. Controller task
    /// ownership remains available for scheduler and Link-Layer progress while
    /// source 127 retains exclusive timer-register authority.
    ///
    /// # Safety
    ///
    /// The caller must retain initialized controller/timer software and an
    /// inactive source-127 CPU route for this exact task owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the initialized timer software and inactive source-127 route"
    )]
    pub unsafe fn prepare_modem_lp_timer_registers(
        &mut self,
    ) -> Result<ModemLpTimerRegistersPreparedOwner, BluetoothModemLpTimerOwnerError> {
        self.reunitable = false;
        // Extract the partition before the first MMIO effect: cancellation or
        // panic can only lose timer authority fail-stop.
        let mut timer = self
            .modem_lp_timer
            .take()
            .ok_or(BluetoothModemLpTimerOwnerError::OwnerSeparated)?;
        timer.prepare_registers();
        Ok(ModemLpTimerRegistersPreparedOwner { timer })
    }
}

/// Why task context cannot perform a modem LP-timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerOwnerError {
    /// The disjoint timer partition has already moved to source-127 storage.
    OwnerSeparated,
}

/// Opaque HAL owner after the source-127 controller-register prefix.
///
/// It deliberately exposes no task/cold escape or rollback:
///
/// ```compile_fail
/// use oer_esp32s31_hal::bluetooth::ModemLpTimerRegistersPreparedOwner;
///
/// fn bypass_remaining_init(prepared: ModemLpTimerRegistersPreparedOwner) {
///     let _task = prepared.into_task();
/// }
/// ```
#[must_use = "the prepared modem LP-timer owner must continue through route setup"]
pub struct ModemLpTimerRegistersPreparedOwner {
    timer: BluetoothModemLpTimerRegisters,
}

impl ModemLpTimerRegistersPreparedOwner {
    /// Complete the low-power hardware component with the retained task owner.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching initialized controller software
    /// environment and must invoke this after the timer-register prefix but
    /// before source 127 can reach the CPU.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the matching controller software and inactive source-127 route"
    )]
    pub unsafe fn initialize_low_power_hardware(
        self,
        task: &mut TaskOwner,
    ) -> ModemLpTimerLowPowerHardwareInitializedOwner {
        task.reunitable = false;
        let mut timer = self.timer;
        let runtime_control = timer.initialize_low_power_hardware();
        ModemLpTimerLowPowerHardwareInitializedOwner {
            timer,
            runtime_control,
        }
    }
}

/// Opaque HAL owner after the complete low-power hardware component.
#[must_use = "the initialized modem LP-timer owner must continue through route setup"]
pub struct ModemLpTimerLowPowerHardwareInitializedOwner {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

impl ModemLpTimerLowPowerHardwareInitializedOwner {
    /// Return the positional runtime-control branch observed during initialization.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Start the runtime timer exactly once for this affine hardware epoch.
    ///
    /// Consuming this owner proves the command cannot be repeated through
    /// the same timer epoch.
    pub fn start_runtime_timer(self) -> ModemLpTimerCounterStartedOwner {
        let mut timer = self.timer;
        timer.start_runtime_counter();
        ModemLpTimerCounterStartedOwner {
            timer,
            runtime_control: self.runtime_control,
        }
    }
}

/// Opaque HAL owner after the one-shot BTDM runtime-timer start command.
#[must_use = "the started modem LP timer must continue through route setup"]
pub struct ModemLpTimerCounterStartedOwner {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

impl ModemLpTimerCounterStartedOwner {
    /// Return the low-power runtime-control branch retained across start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Transfer the unique started timer into stable source-127 ISR storage.
    ///
    /// This transition performs no MMIO and exposes no raw PAC owner. The
    /// platform must store the returned value before enabling the CPU route
    /// and recover it only after that route is disabled and no hard handler
    /// remains in flight.
    pub fn stage_for_interrupt(self) -> ModemLpTimerInterruptReadyOwner {
        ModemLpTimerInterruptReadyOwner { timer: self.timer }
    }
}

/// Opaque HAL owner staged for the source-127 hard handler.
#[must_use = "the modem LP-timer interrupt owner must remain in stable ISR storage"]
pub struct ModemLpTimerInterruptReadyOwner {
    timer: BluetoothModemLpTimerRegisters,
}

/// Result of one bounded source-127 hard-handler register step.
#[must_use = "retain the ready owner or complete the required software handler"]
pub enum ModemLpTimerInterruptStep {
    /// `WORD_0038_OPAQUE` was zero and the owner is ready for a later IRQ entry.
    Spurious(ModemLpTimerInterruptReadyOwner),
    /// The reviewed path requires the common software timer handler.
    HandlerPending(ModemLpTimerHandlerPendingOwner),
}

/// Opaque HAL owner for the common modem timer handler's register phase.
///
/// No direct rearm or task-owner escape exists. The owner must execute the
/// bounded register step, which either rearms an idle handler or produces the
/// separate fail-closed software-pending state.
#[must_use = "the common modem LP-timer register phase remains pending"]
pub struct ModemLpTimerHandlerPendingOwner {
    timer: BluetoothModemLpTimerRegisters,
    observation: BluetoothModemLpTimerInterruptObservation,
}

impl ModemLpTimerHandlerPendingOwner {
    /// Return the exact positional path that selected handler dispatch.
    pub const fn observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.observation
    }

    /// Execute the bounded register-acknowledgement phase of the common timer
    /// handler without invoking software or an RTOS service.
    pub fn step_registers(self) -> ModemLpTimerHandlerRegisterStep {
        let mut timer = self.timer;
        match timer.acknowledge_handler_registers() {
            None => {
                ModemLpTimerHandlerRegisterStep::Rearmed(ModemLpTimerInterruptReadyOwner { timer })
            }
            Some(register_observation) => {
                ModemLpTimerHandlerRegisterStep::SoftwarePending(ModemLpTimerSoftwarePendingOwner {
                    timer,
                    interrupt_observation: self.observation,
                    register_observation,
                })
            }
        }
    }
}

/// Result of the common handler's bounded register-acknowledgement phase.
#[must_use = "retain the ready owner or complete the required software work"]
pub enum ModemLpTimerHandlerRegisterStep {
    /// No software work was requested and the interrupt owner is ready again.
    Rearmed(ModemLpTimerInterruptReadyOwner),
    /// At least one acknowledged state byte requires software work.
    SoftwarePending(ModemLpTimerSoftwarePendingOwner),
}

/// Opaque HAL owner retained while source-127 software work is pending.
///
/// This owner has no raw PAC escape and cannot be rearmed until the no-RTOS
/// timer runtime supplies the missing software transition and final register
/// read.
#[must_use = "software timer work and the final hardware read remain pending"]
pub struct ModemLpTimerSoftwarePendingOwner {
    timer: BluetoothModemLpTimerRegisters,
    interrupt_observation: BluetoothModemLpTimerInterruptObservation,
    register_observation: BluetoothModemLpTimerHandlerRegisterObservation,
}

impl ModemLpTimerSoftwarePendingOwner {
    /// Return the initial source-127 classifier path.
    pub const fn interrupt_observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.interrupt_observation
    }

    /// Return the positional state bytes requiring software consequences.
    pub const fn register_observation(&self) -> BluetoothModemLpTimerHandlerRegisterObservation {
        self.register_observation
    }

    /// Sample one finite positional LP-timer instant and acknowledge a newly
    /// observed rollover without polling.
    pub fn sample_counter(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCounterObservation {
        self.timer.sample_counter(epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        self.timer.disable_compare();
    }

    /// Program one positional deadline and return the exact hardware branch.
    pub fn program_compare(
        &mut self,
        deadline: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCompareDisposition {
        self.timer.program_compare(deadline, epoch)
    }

    /// Perform the final fresh handler read and return the ISR-ready owner.
    ///
    /// This seam is hidden because the controller timer state machine must call
    /// it only after every software consequence represented by this owner has
    /// completed.
    #[doc(hidden)]
    pub fn complete_software(self) -> ModemLpTimerInterruptReadyOwner {
        let mut timer = self.timer;
        timer.sample_final_state();
        ModemLpTimerInterruptReadyOwner { timer }
    }
}

impl ModemLpTimerInterruptReadyOwner {
    /// Perform one finite source-127 register classification.
    ///
    /// The unique ISR-staged owner is the authority for exactly one register
    /// pass. The method never waits, loops, allocates or invokes an RTOS
    /// service.
    pub fn step(self) -> ModemLpTimerInterruptStep {
        let mut timer = self.timer;
        match timer.classify_interrupt() {
            None => ModemLpTimerInterruptStep::Spurious(ModemLpTimerInterruptReadyOwner { timer }),
            Some(observation) => {
                ModemLpTimerInterruptStep::HandlerPending(ModemLpTimerHandlerPendingOwner {
                    timer,
                    observation,
                })
            }
        }
    }
}

/// Why the opaque HAL task owner cannot return to cold ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOwnerReuniteError {
    /// A mutable controller or shared-PHY capability was issued and complete
    /// hardware rollback has not returned it to the cold baseline.
    HardwareLifecycleNotRestored,
    /// Interrupt output setup or routing touched hardware and the complete
    /// interrupt-bank baseline has not been restored.
    InterruptLifecycleNotRestored,
    /// A controller-time request still belongs to the task-side worker.
    ControllerTimeLatchInFlight,
    /// Source 127 still retains the disjoint modem LP-timer owner.
    ModemLpTimerOwnerSeparated,
}

/// Failed task/IRQ reunion retaining both opaque HAL owners unchanged.
#[must_use = "failed Bluetooth reunion still owns both HAL partitions"]
pub struct TaskOwnerReuniteFailure {
    task: TaskOwner,
    interrupts: InterruptSetupOwner,
    error: TaskOwnerReuniteError,
}

impl TaskOwnerReuniteFailure {
    /// Return the finite reunion failure reason.
    pub const fn error(&self) -> TaskOwnerReuniteError {
        self.error
    }

    /// Recover both retained HAL owners and the failure reason.
    pub fn into_parts(self) -> (TaskOwner, InterruptSetupOwner, TaskOwnerReuniteError) {
        (self.task, self.interrupts, self.error)
    }
}

impl core::fmt::Debug for TaskOwnerReuniteFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskOwnerReuniteFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Opaque inactive owner of the controller interrupt partition.
#[must_use = "the inactive Bluetooth interrupt owner must be staged or reunited"]
pub struct InterruptSetupOwner {
    registers: PacBluetoothInterruptSetup,
    reunitable: bool,
}

impl InterruptSetupOwner {
    /// Execute the reviewed baseline clear/enable/output preparation.
    ///
    /// The controller lifecycle owns completed HAL-init and quiescent dynamic
    /// sources. The returned state remains retained until stable ISR storage
    /// is ready.
    ///
    /// # Safety
    ///
    /// The caller must retain completed controller HAL-init, quiescent dynamic
    /// sources, and inactive primary/NRT CPU routes for this interrupt owner.
    #[allow(
        unsafe_code,
        reason = "the caller must retain completed controller init and inactive CPU routes"
    )]
    pub unsafe fn prepare_controller_output(self) -> InterruptOutputPreparedOwner {
        InterruptOutputPreparedOwner {
            registers: self.registers.prepare_controller_output(),
        }
    }
}

/// Controller IRQ output prepared but not yet transferred into stable ISR
/// storage or bound to a CPU route.
#[must_use = "the prepared Bluetooth output must be staged or released"]
pub struct InterruptOutputPreparedOwner {
    registers: oer_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

impl InterruptOutputPreparedOwner {
    /// Transfer the register partition to the state required by shared ISR
    /// storage before either platform route is enabled.
    pub fn stage_for_cpu_routes(self) -> InterruptRegistersOwner {
        InterruptRegistersOwner {
            registers: self.registers.stage_for_cpu_routes(),
        }
    }

    /// Execute the reviewed controller-output release transaction after CPU
    /// routes have been removed.
    ///
    /// Dynamic Link-Layer sources must already be quiescent. This transaction
    /// alone is not controller, packet, BTBB, PHY or clock teardown. Even on
    /// the never-routed rollback path, the returned setup owner is marked
    /// non-pristine and cannot reconstruct the neutral hardware root.
    pub fn release_controller_output(self) -> InterruptSetupOwner {
        InterruptSetupOwner {
            registers: self.registers.release_controller_output(),
            reunitable: false,
        }
    }
}

/// Opaque interrupt-register owner staged for primary and NRT CPU routes.
#[must_use = "the staged Bluetooth interrupt owner must be deactivated"]
pub struct InterruptRegistersOwner {
    registers: BluetoothInterruptRegisters,
}

impl InterruptRegistersOwner {
    /// Prepare the dynamic interrupt groups required immediately before a
    /// scheduler run publication.
    pub fn prepare_scheduler_run_interrupts(&mut self) -> BluetoothSchedulerRunInterruptsPrepared {
        self.registers.prepare_scheduler_run_interrupts()
    }

    /// Capture and acknowledge one complete opaque NRT source-133 epoch.
    ///
    /// This preserves the PAC sample/sample/acknowledge/acknowledge order. It
    /// deliberately performs no feature dispatch and publishes no task wake.
    pub fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
        self.registers.capture_nrt_and_acknowledge()
    }

    /// Capture, acknowledge and retain one complete primary source-124 epoch.
    pub fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.registers.capture_primary_and_acknowledge()
    }

    /// Sample scheduler BUSY through the diagnostic pair for the bank-one
    /// source-3 reference gate, within `budget` attempts.
    pub fn capture_scheduler_reference_gate(
        &mut self,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, BluetoothDiagnosticUnsettled> {
        diagnostic::settle(budget, || self.registers.capture_scheduler_reference_gate())
    }

    /// Clear the scheduler reference selected by the source-124 gate.
    ///
    /// The returned affine token proves the ordered PAC write and device fence.
    /// The open scheduler's software consistency is structural and does not
    /// reproduce the vendor's intrusive-list callback.
    pub fn clear_scheduler_reference(&mut self) -> BluetoothSchedulerReferenceCleared {
        self.registers.clear_scheduler_reference()
    }

    /// Capture the later, independent scheduler-state observation used to
    /// construct deferred work.
    pub fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.registers.capture_scheduler_work()
    }

    /// Sample scheduler BUSY through the diagnostic pair for one task-side
    /// scheduler decision, within `budget` attempts.
    pub fn capture_scheduler_busy(
        &mut self,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<BluetoothSchedulerBusyObservation, BluetoothDiagnosticUnsettled> {
        diagnostic::settle(budget, || self.registers.capture_scheduler_busy())
    }

    /// Acknowledge interrupt source 7 for one indexed scheduler
    /// cancellation, between its index publication and its control request.
    pub fn acknowledge_scheduler_cancellation_source(
        &mut self,
        indexed: &BluetoothSchedulerCancellationIndexed,
    ) -> BluetoothSchedulerCancellationSourceAcknowledged {
        self.registers
            .acknowledge_scheduler_cancellation_source(indexed)
    }

    /// Sample scheduler BUSY through the interrupt-owned diagnostic pair for
    /// one lock/modify decision without borrowing task-side controller
    /// registers, within `budget` attempts.
    pub fn capture_scheduler_lock_modify_interrupt(
        &mut self,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<BluetoothSchedulerLockModifyInterruptObservation, BluetoothDiagnosticUnsettled>
    {
        diagnostic::settle(budget, || {
            self.registers.capture_scheduler_lock_modify_interrupt()
        })
    }

    /// Return the register partition to output-prepared ownership after both
    /// CPU routes have been disabled and shared ISR access has ended.
    ///
    /// The returned state requires the terminal idle/head/fault preflight
    /// before it can release Controller output.
    pub fn deactivate(self) -> InterruptOutputAfterRoutesOwner {
        InterruptOutputAfterRoutesOwner {
            _registers: self.registers.deactivate(),
        }
    }
}

/// Controller interrupt bank recovered from stable ISR storage after both CPU
/// routes were disabled.
///
/// Checked output release requires the matching task registers and fresh
/// scheduler/head/fault observations. No direct setup or cold conversion exists.
#[must_use = "post-route interrupt ownership awaits dynamic-source quiescence"]
pub struct InterruptOutputAfterRoutesOwner {
    _registers: oer_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

impl InterruptOutputAfterRoutesOwner {
    /// Sample scheduler BUSY within `budget` attempts for one release step.
    fn capture_scheduler_busy(
        &self,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<BluetoothSchedulerBusyObservation, BluetoothControllerOutputReleaseError> {
        diagnostic::settle(budget, || self._registers.capture_scheduler_busy()).map_err(
            |BluetoothDiagnosticUnsettled| {
                BluetoothControllerOutputReleaseError::SchedulerBusyUnsettled
            },
        )
    }

    /// Observe quiescence while retaining the prepared, unrouted output bank.
    pub fn validate_idle_controller(
        &self,
        task: &mut TaskOwner,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        if task.time_latch.in_flight() {
            return Err(BluetoothControllerOutputReleaseError::ControllerTimePending);
        }
        let busy = self.capture_scheduler_busy(budget)?;
        self._registers
            .validate_idle_controller(&mut task.registers, busy)
    }

    /// Revalidate quiescence after maintenance and return the same prepared bank.
    /// This does not enable CPU routes or publish either ISR owner.
    pub fn try_reactivate_idle_controller_output(
        self,
        task: &mut TaskOwner,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<InterruptRegistersOwner, (BluetoothControllerOutputReleaseError, Self)> {
        if let Err(error) = self.validate_idle_controller(task, budget) {
            return Err((error, self));
        }
        Ok(InterruptRegistersOwner {
            registers: self._registers.stage_for_cpu_routes(),
        })
    }

    /// Mask and acknowledge the idle scheduler's dynamic sources, then release
    /// Controller output. A busy scheduler, published head, pending time latch,
    /// unsettled BUSY sample or primary fault retains both owners. Rejection
    /// can leave RUN disabled; it never authorizes reactivation, memory
    /// reclamation or cold reunion.
    ///
    /// BUSY is sampled twice within `budget` attempts each: before the
    /// dynamic sources are masked and after the fence that disables RUN.
    pub fn try_release_idle_controller_output(
        self,
        task: &mut TaskOwner,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<InterruptOutputReleasedOwner, (BluetoothControllerOutputReleaseError, Self)> {
        if task.time_latch.in_flight() {
            return Err((
                BluetoothControllerOutputReleaseError::ControllerTimePending,
                self,
            ));
        }
        let quiesced = match self.capture_scheduler_busy(budget).and_then(|busy| {
            self._registers
                .quiesce_idle_controller_output(&mut task.registers, busy)
        }) {
            Ok(quiesced) => quiesced,
            Err(error) => return Err((error, self)),
        };
        let busy = match self.capture_scheduler_busy(budget) {
            Ok(busy) => busy,
            Err(error) => return Err((error, self)),
        };
        match self._registers.try_release_idle_controller_output(
            &mut task.registers,
            quiesced,
            busy,
        ) {
            Ok(registers) => Ok(InterruptOutputReleasedOwner { registers }),
            Err((error, registers)) => Err((
                error,
                Self {
                    _registers: registers,
                },
            )),
        }
    }
}

/// Output released after the post-route, idle-scheduler hardware preflight.
/// No active IRQ or setup conversion is exposed; retain it through RF close.
#[must_use = "released output must remain paired with its stopped Controller"]
pub struct InterruptOutputReleasedOwner {
    registers: PacBluetoothInterruptSetup,
}

/// Exclusive finite borrow of the Bluetooth controller task-side registers.
///
/// This type deliberately exposes neither `Deref`, a raw PAC accessor nor a
/// constructor. New operations belong here only after their PAC transaction
/// and lifecycle prerequisites are independently bounded.
pub struct ControllerHal<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
    time_latch: &'registers mut ControllerTimeLatch,
}

/// Public Bluetooth Controller identity in canonical display order.
///
/// Keeping the canonical representation in this type makes the sole reversal
/// into the Controller's least-significant-octet-first order an internal HAL
/// concern. It cannot be confused with a random address already decoded from
/// an HCI command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerPublicAddress {
    canonical_octets: [u8; 6],
}

impl ControllerPublicAddress {
    /// Construct a public identity from canonical EUI-48 display order.
    pub const fn from_canonical_bytes(canonical_octets: [u8; 6]) -> Self {
        Self { canonical_octets }
    }

    /// Return the canonical EUI-48 display order unchanged.
    pub const fn canonical_bytes(self) -> [u8; 6] {
        self.canonical_octets
    }

    const fn controller_wire_octets(self) -> [u8; 6] {
        let [octet_0, octet_1, octet_2, octet_3, octet_4, octet_5] = self.canonical_octets;
        [octet_5, octet_4, octet_3, octet_2, octet_1, octet_0]
    }
}

/// Random Bluetooth Controller identity in HCI/Link-Layer wire order.
///
/// `LE Set Random Address` already carries the least-significant address octet
/// first. This distinct type prevents that representation from being reversed
/// a second time before publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerRandomAddress {
    hci_wire_octets: [u8; 6],
}

impl ControllerRandomAddress {
    /// Construct a random identity from an HCI `BD_ADDR` payload.
    pub const fn from_hci_wire_bytes(hci_wire_octets: [u8; 6]) -> Self {
        Self { hci_wire_octets }
    }

    /// Return the HCI/Link-Layer order unchanged.
    pub const fn hci_wire_bytes(self) -> [u8; 6] {
        self.hci_wire_octets
    }

    const fn controller_wire_octets(self) -> [u8; 6] {
        self.hci_wire_octets
    }
}

/// One initialized receive-memory list published to the controller.
///
/// The token is intentionally affine. It records the positional list and its
/// validated head without granting access to either memory or MMIO. The
/// memory owner must retain the complete pinned graph until a later verified
/// retirement transaction consumes this publication.
#[must_use = "the published receive list remains owned by the controller"]
pub struct RxMemoryListPublished {
    selector: BluetoothMemoryListSelector,
    head: BluetoothControllerSramAddress,
}

/// HAL ownership of the controller-global disabled-CTE publication.
///
/// The restricted PAC proof remains private, so upper Controller layers can
/// retain the hardware epoch without depending on a register-level type.
#[must_use = "the disabled-CTE publication must retain its pinned workspace"]
pub struct DirectionFindingDisabledBaselineOwner {
    _prepared: BluetoothDirectionFindingDisabledBaselinePrepared,
}

impl RxMemoryListPublished {
    /// Return the positional hardware-list selector chosen by the memory
    /// layer.
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        self.selector
    }

    /// Return the validated first-node address without granting dereference
    /// access.
    pub const fn head(&self) -> BluetoothControllerSramAddress {
        self.head
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RxPacketControl {
    ControllerDefault,
    SoftwareConnection,
}

trait RxMemoryListInitialPublication {
    fn prepare_packet_control(&mut self, policy: RxPacketControl);
    fn publish_current_head(&mut self);
    fn clear_next_head(&mut self);
    fn reset_initial_control(&mut self);
}

fn execute_rx_memory_list_initial_publication(
    transaction: &mut impl RxMemoryListInitialPublication,
    policy: RxPacketControl,
) {
    transaction.prepare_packet_control(policy);
    transaction.publish_current_head();
    transaction.clear_next_head();
    transaction.reset_initial_control();
}

struct PacBluetoothRxMemoryListInitialPublication<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
    selector: BluetoothMemoryListSelector,
    head: BluetoothControllerSramAddress,
}

impl RxMemoryListInitialPublication for PacBluetoothRxMemoryListInitialPublication<'_> {
    fn prepare_packet_control(&mut self, policy: RxPacketControl) {
        match policy {
            RxPacketControl::ControllerDefault => {
                self.registers.restore_default_connection_packet_control();
            }
            RxPacketControl::SoftwareConnection => {
                self.registers.prepare_software_connection_packet_control();
            }
        }
    }

    #[allow(
        unsafe_code,
        reason = "the transaction retains the serialized RX-list lifecycle"
    )]
    fn reset_initial_control(&mut self) {
        // SAFETY: the enclosing operation retains the same pinned graph and
        // exclusive list lifecycle through pointer publication and this reset.
        unsafe {
            self.registers
                .reset_memory_list_initial_control(self.selector)
        };
    }

    #[allow(
        unsafe_code,
        reason = "the enclosing HAL operation retains list lifetime and controller-lifecycle prerequisites"
    )]
    fn publish_current_head(&mut self) {
        // SAFETY: this transaction exists only inside the two unsafe RX-list
        // initial-publication operations, whose callers retain the pinned
        // list at `head` and the serialized controller lifecycle.
        unsafe {
            self.registers.program_memory_list_pointer(
                self.selector,
                BluetoothMemoryListSlot::CurrentRx,
                BluetoothMemoryListPointerImage::Address(self.head),
            );
        }
    }

    #[allow(
        unsafe_code,
        reason = "the enclosing HAL operation retains list lifetime and controller-lifecycle prerequisites"
    )]
    fn clear_next_head(&mut self) {
        // SAFETY: as for `publish_current_head`; a zero next pointer
        // references no storage.
        unsafe {
            self.registers.program_memory_list_pointer(
                self.selector,
                BluetoothMemoryListSlot::NextRx,
                BluetoothMemoryListPointerImage::Zero,
            );
        }
    }
}

impl ControllerHal<'_> {
    /// Publish the public identity through the Controller's fixed public slot.
    ///
    /// The owning lifecycle must retain the powered BLE Controller epoch and
    /// prove that no active advertising, scanning or connection operation can
    /// consume the address pair during replacement. Cold start must invoke
    /// this after BLE PHY initialization and before interrupt publication or
    /// radio work.
    #[doc(hidden)]
    pub fn program_public_device_address(&mut self, address: ControllerPublicAddress) {
        self.registers
            .program_bluetooth_public_device_address(address.controller_wire_octets());
    }

    /// Publish the selected random identity through its fixed Controller slot.
    ///
    /// The owning lifecycle must retain the powered BLE Controller epoch and
    /// prove that no active advertising, scanning or connection operation can
    /// consume the address pair during replacement. A role-start path must
    /// perform this while idle and before publishing its scheduler head or
    /// `RUN`.
    #[doc(hidden)]
    pub fn program_random_device_address(&mut self, address: ControllerRandomAddress) {
        self.registers
            .program_bluetooth_random_device_address(address.controller_wire_octets());
    }

    /// Publish the controller-global CTE-disabled descriptor baseline.
    ///
    /// # Safety
    ///
    /// The caller must retain the powered Controller epoch and the matching
    /// initialized pinned descriptor until a reviewed retirement transition.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains powered-controller and descriptor-lifetime prerequisites"
    )]
    pub unsafe fn prepare_direction_finding_disabled_baseline(
        &mut self,
        descriptor: BluetoothControllerSramAddress,
    ) -> DirectionFindingDisabledBaselineOwner {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        let prepared = unsafe {
            self.registers
                .prepare_direction_finding_disabled_baseline(descriptor)
        };
        DirectionFindingDisabledBaselineOwner {
            _prepared: prepared,
        }
    }

    /// Publish one initialized receive-memory list in the reviewed cold order.
    ///
    /// The memory layer owns the semantic mapping from a controller role to
    /// the positional `selector`. HAL publishes the initialized current head
    /// first and only then clears the matching next head. Neither positional
    /// slot nor its register representation crosses this boundary.
    /// The controller-default opcode policy is restored before either pointer;
    /// software-owned connections use their separate publication transaction.
    ///
    /// # Safety
    ///
    /// The caller must own the selected powered-controller lifecycle epoch,
    /// must have no active RUN, retain the complete initialized pinned memory graph, and
    /// serialize all task and interrupt access to this list until a later
    /// verified retirement transaction consumes the returned token.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains pinned-list lifetime and controller-lifecycle prerequisites"
    )]
    pub unsafe fn publish_rx_memory_list_initial_head(
        &mut self,
        selector: BluetoothMemoryListSelector,
        head: BluetoothControllerSramAddress,
    ) -> RxMemoryListPublished {
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::ControllerDefault,
        );
        RxMemoryListPublished { selector, head }
    }

    /// Publish RX for a connection whose control PDUs are owned by software.
    ///
    /// Configure opcode inspection before publishing the RX head. Software CCM
    /// ciphertext must reach the authenticating LL decoder even when its first
    /// byte matches a hardware abort opcode. Repeating this transaction also
    /// restores the policy after PHY maintenance reinitializes BLE registers.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned connection graph and the
    /// exclusive powered task epoch, with no active RUN. It must serialize RX
    /// list access until the returned publication is retired.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the connection owner retains the powered graph and idle hardware"
    )]
    pub unsafe fn publish_software_connection_rx_memory_list_initial_head(
        &mut self,
        selector: BluetoothMemoryListSelector,
        head: BluetoothControllerSramAddress,
    ) -> RxMemoryListPublished {
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::SoftwareConnection,
        );
        RxMemoryListPublished { selector, head }
    }

    /// Publish one complete reviewed scanner command transaction.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned scanner graph, its
    /// matching RX-list publication and an exclusive powered controller epoch.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains scanner graph lifetime and controller-lifecycle prerequisites"
    )]
    pub unsafe fn publish_scan_start(&mut self) -> BluetoothScanStartPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_scan_start() }
    }

    /// Publish the device table hardware filters advertising against: its
    /// entry count, then the address of its first entry.
    ///
    /// # Safety
    ///
    /// The caller must retain `count` initialized, pinned entries from
    /// `first_entry` and an exclusive powered controller epoch, and must
    /// serialize this publication with every role that filters against the
    /// table.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains table lifetime and controller-lifecycle prerequisites"
    )]
    pub unsafe fn publish_device_table(
        &mut self,
        first_entry: BluetoothControllerSramAddress,
        count: BluetoothDeviceTableEntryCount,
    ) -> BluetoothDeviceTablePublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_device_table(first_entry, count) }
    }

    /// Disable the BLE PHY ETM route for one Direct Test Mode event without
    /// CTE, as the pinned DTM event bodies do.
    pub fn disable_ble_phy_etm_route(&mut self) -> BlePhyEtmRouteDisabled {
        self.registers.disable_ble_phy_etm_route()
    }

    /// Route and enable the BLE PHY ETM channel again after a test.
    pub fn restore_ble_phy_etm_route(&mut self, route: BlePhyEtmRouteDisabled) {
        self.registers.restore_ble_phy_etm_route(route);
    }

    /// Remove every published scheduler hardware-list head.
    ///
    /// This is only the reviewed controller-initialization prefix. It does not
    /// establish scheduler, Link Layer or HCI readiness. The borrow proves
    /// exclusive register access, not powered lifecycle state; the caller must
    /// retain the independently established clock/reset prerequisite.
    pub fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        self.registers.clear_scheduler_hardware_list_heads()
    }

    /// Perform one fresh fenced post-completion head observation while
    /// retaining the exact affine RUN provenance on every result path.
    pub fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerHardwareListHeadRetirementObservation {
        self.registers
            .observe_scheduler_hardware_list_head_retirement(run)
    }

    /// Read the current head of one hardware list once, followed by a
    /// device fence.
    pub fn observe_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        self.registers.observe_scheduler_hardware_list_head(index)
    }

    /// Order prior descriptor writes and publish one scheduler hardware-list
    /// head through the restricted PAC.
    ///
    /// # Safety
    ///
    /// The caller must retain the scheduler insertion epoch, completed
    /// descriptor initialization, graph lifetime and task/interrupt
    /// serialization required by the underlying PAC transaction. The PAC
    /// orders those prior SRAM writes before the MMIO head update.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler-epoch prerequisites"
    )]
    pub unsafe fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe {
            self.registers
                .publish_scheduler_hardware_list_head(index, head)
        }
    }

    /// Clear START in the insertion command selected by the Controller.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching insertion-begin result and must
    /// serialize the command RMW with every producer and interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the insertion result and command serialization"
    )]
    pub unsafe fn clear_scheduler_insertion_command_start(
        &mut self,
        command: BluetoothSchedulerInsertionCommand,
    ) -> BluetoothSchedulerInsertionCommandStartCleared {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe {
            self.registers
                .clear_scheduler_insertion_command_start(command)
        }
    }

    /// Publish one scheduler skip request through the restricted PAC.
    ///
    /// # Safety
    ///
    /// The request must name an initialized item linked in that hardware
    /// list. The caller must retain the pinned item and exclusive list
    /// ownership until the request is observed and cleared.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the listed item lifetime and list serialization"
    )]
    pub unsafe fn publish_scheduler_skip(
        &mut self,
        request: BluetoothSchedulerSkipRequest,
    ) -> BluetoothSchedulerSkipPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_scheduler_skip(request) }
    }

    /// Perform one finite skip-request observation.
    pub fn observe_scheduler_skip(
        &mut self,
        published: &BluetoothSchedulerSkipPublished,
        scheduler: BluetoothSchedulerBusyObservation,
    ) -> BluetoothSchedulerSkipDisposition {
        self.registers.observe_scheduler_skip(published, scheduler)
    }

    /// Clear a skip request after its terminal observation.
    pub fn clear_scheduler_skip(
        &mut self,
        published: BluetoothSchedulerSkipPublished,
    ) -> BluetoothSchedulerSkipCleared {
        self.registers.clear_scheduler_skip(published)
    }

    /// Publish the scheduler cancellation hardware-list index.
    ///
    /// # Safety
    ///
    /// The caller must have observed the lock/modify request idle and must
    /// retain exclusive list ownership until the cancellation control is
    /// released.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller serializes the shared operational word"
    )]
    pub unsafe fn index_scheduler_cancellation(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerCancellationIndexed {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.index_scheduler_cancellation(index) }
    }

    /// Set the scheduler cancellation control after its index and source
    /// acknowledgement.
    pub fn request_scheduler_cancellation(
        &mut self,
        indexed: BluetoothSchedulerCancellationIndexed,
        acknowledged: BluetoothSchedulerCancellationSourceAcknowledged,
    ) -> BluetoothSchedulerCancellationRequested {
        self.registers
            .request_scheduler_cancellation(indexed, acknowledged)
    }

    /// Perform one finite observation of the cancellation control waits.
    pub fn observe_scheduler_cancellation(
        &mut self,
        requested: &mut BluetoothSchedulerCancellationRequested,
        scheduler: BluetoothSchedulerBusyObservation,
    ) -> BluetoothSchedulerCancellationDisposition {
        self.registers
            .observe_scheduler_cancellation(requested, scheduler)
    }

    /// Clear the scheduler cancellation control.
    pub fn release_scheduler_cancellation(
        &mut self,
        requested: BluetoothSchedulerCancellationRequested,
    ) -> BluetoothSchedulerCancellationReleased {
        self.registers.release_scheduler_cancellation(requested)
    }

    /// Publish the synchronous BTMAC scheduler event through the restricted
    /// PAC after the matching head and dynamic interrupts are prepared.
    #[doc(hidden)]
    pub fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        self.registers.publish_scheduler_run_event(head, interrupts)
    }

    /// Publish the final scheduler hardware RUN command through the
    /// restricted PAC. The consumed event retains the complete run prologue.
    #[doc(hidden)]
    pub fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        self.registers.publish_scheduler_hardware_run_command(event)
    }

    /// Capture task-owned START and RESULT fields for one scheduler
    /// lock/modify decision.
    pub fn capture_scheduler_lock_modify_task(
        &mut self,
    ) -> BluetoothSchedulerLockModifyTaskObservation {
        self.registers.capture_scheduler_lock_modify_task()
    }

    /// Finish one finite task-owned software-list removal observation after a
    /// fresh interrupt-side idle sample.
    pub fn finish_scheduler_software_list_removal(
        &mut self,
        idle: BluetoothSchedulerSoftwareListRemovalIdle,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> BluetoothSchedulerSoftwareListRemovalJoin {
        self.registers
            .finish_scheduler_software_list_removal(idle, head)
    }

    /// Advance the common stop preamble/request/idle sequence under interrupt
    /// serialization. Pending retains the request; the caller owns its deadline.
    /// Each BUSY sample takes at most `budget` attempts.
    pub fn step_scheduler_stop(
        &mut self,
        interrupts: &mut InterruptRegistersOwner,
        stop: BluetoothSchedulerStop,
        budget: BluetoothDiagnosticReadBudget,
    ) -> Result<BluetoothSchedulerStopStep, BluetoothDiagnosticUnsettled> {
        scheduler_stop::step_hardware(self.registers, &mut interrupts.registers, stop, budget)
    }

    /// Retire the exact stopped RUN head without clearing a foreign item.
    pub fn retire_stopped_scheduler_head(
        &mut self,
        stopped: BluetoothSchedulerStopped,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerStoppedHeadRetirement {
        self.registers.retire_stopped_scheduler_head(stopped, run)
    }

    /// Transfer one fresh hardware finished-list observation to its reviewed
    /// report register and return the typed sixteen-list projection.
    ///
    /// This finite task-side operation performs no item lookup or callback.
    /// The Controller must retain descriptor ownership until a separately
    /// proven completion-visibility fence permits CPU access.
    pub fn transfer_scheduler_finished_lists(
        &mut self,
    ) -> BluetoothSchedulerFinishedListObservation {
        self.registers.transfer_scheduler_finished_lists()
    }

    /// Execute the finite scheduler lock/modify publication transaction.
    ///
    /// The Bluetooth controller state machine keeps this method hidden behind
    /// its affine publication token. It performs two fresh operational-word
    /// RMW edges, publishes the typed request and returns only after a device
    /// fence; it never polls or blocks an executor.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned scheduler item named by
    /// `request` and exclusive scheduler task/interrupt serialization until
    /// verified Controller ownership return.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler serialization"
    )]
    pub unsafe fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        // SAFETY: this method forwards the identical descriptor-lifetime and
        // serialization obligations to the restricted PAC transaction.
        unsafe { self.registers.publish_scheduler_lock_modify(request) }
    }

    /// Publish one controller-time latch request and return immediately.
    ///
    /// The powered lifecycle must first establish a reset/quiescent timer
    /// domain. Entering the Bluetooth ownership route alone performs no reset
    /// and deliberately does not treat an arbitrary hardware bit image as a
    /// fresh request owned by this driver.
    ///
    /// The unique task owner remembers the request across HAL borrows. If an
    /// async operation is cancelled before `Ready`, another begin fails closed
    /// and the durable task owner must drain that same request with
    /// [`Self::step_controller_time_latch`] before admitting new work.
    pub fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        controller_time::begin(self.time_latch, self.registers)
    }

    /// Perform exactly one observation of the controller-time latch.
    ///
    /// `Waiting` means hardware still owns the request and the caller should
    /// yield until an interrupt or bounded timer event. This method never
    /// loops, registers a waker, allocates or depends on an RTOS.
    pub fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        controller_time::step(self.time_latch, self.registers)
    }

    /// Whether this task owner retains an unfinished latch request.
    ///
    /// This is a diagnostic view for the durable controller task owner. A
    /// cancelled logical operation must be drained before a fresh request is
    /// begun; its sample must not be relabelled as that fresh request.
    pub fn controller_time_latch_in_flight(&self) -> bool {
        self.time_latch.in_flight()
    }
}

mod sealed {

    use crate::bluetooth::TaskOwner;

    pub trait ControllerHalBorrow {
        fn bluetooth_task_owner_mut(&mut self) -> &mut TaskOwner;
    }

    impl ControllerHalBorrow for TaskOwner {
        fn bluetooth_task_owner_mut(&mut self) -> &mut TaskOwner {
            self
        }
    }
}

/// Sealed conversion from the exclusive PAC task owner to one finite
/// controller HAL borrow.
///
/// This conversion proves aliasing only. It deliberately does not manufacture
/// a powered-controller typestate; production operations remain sequenced by
/// the Bluetooth lifecycle owner above this borrow.
///
/// The borrow follows ordinary Rust exclusivity. For example, two simultaneous
/// controller borrows cannot be created:
///
/// ```compile_fail
/// use oer_esp32s31_hal::bluetooth::ControllerHalBorrow;
///
/// fn duplicate(owner: &mut impl ControllerHalBorrow) {
///     let first = owner.borrow_bluetooth_controller();
///     let second = owner.borrow_bluetooth_controller();
///     let _ = (first, second);
/// }
/// ```
#[doc(hidden)]
pub trait ControllerHalBorrow: sealed::ControllerHalBorrow {
    /// Borrow the controller registers without exposing their PAC owner.
    fn borrow_bluetooth_controller(&mut self) -> ControllerHal<'_> {
        let owner = sealed::ControllerHalBorrow::bluetooth_task_owner_mut(self);
        owner.reunitable = false;
        ControllerHal {
            registers: &mut owner.registers,
            time_latch: &mut owner.time_latch,
        }
    }
}

impl ControllerHalBorrow for TaskOwner {}

#[cfg(test)]
mod tests;
