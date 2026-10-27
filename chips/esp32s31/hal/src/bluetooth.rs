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

use crate::power::PlatformClockBackend;

use core::fmt;

use registers::{
    BluetoothDirectionFindingDisabledBaselinePrepared, BluetoothInterruptRegisters,
    BluetoothInterruptSetup as RegistersBluetoothInterruptSetup, BluetoothModemLpTimerRegisters,
    BluetoothPhyRegisterInitInputs, BluetoothTaskRegisters,
};

use crate::domain::{
    BluetoothClient, BtbbAcquired, BtbbError, CoexAbsent, CommonPower, CommonPowerExitFailure,
    CommonRadioPowerError, LowPowerClockError, ModemClockError, PlatformClockProvider, RadioDomain,
};
pub use registers::{BluetoothControllerParts, BluetoothModemLpTimerParts};

mod controller_time;
pub(crate) mod registers;
mod scheduler_execution_lock;
mod scheduler_execution_modify;
mod scheduler_stop;
mod shutdown;

use controller_time::ControllerTimeLatch;
pub use controller_time::{
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchRequest,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
};
use oer_time::settle::{self, Settle, Unsettled};
pub use scheduler_execution_lock::BluetoothSchedulerExecutionLock;
pub use scheduler_execution_modify::BluetoothSchedulerExecutionModify;
pub use scheduler_stop::BluetoothSchedulerStop;
pub use shutdown::{
    BluetoothControllerReset, BluetoothControllerResetTimerPending, BluetoothShutdownError,
    BluetoothShutdownFailure,
};

pub use crate::modem::{
    BluetoothLowPowerClockObservation, ModemSysconBluetoothObservation, SharedModemClockObservation,
};
pub use registers::{
    BlePhyEtmRouteDisabled, BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
    BluetoothControllerOutputReleaseError, BluetoothControllerTimeScale,
    BluetoothDeviceTablePublished, BluetoothHalInitPeriod, BluetoothHalInitScale,
    BluetoothLowPowerRuntimeControlObservation, BluetoothMemoryListSelector,
    BluetoothMemoryListSlot, BluetoothMicrosecondDeltaProjection,
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerCounterObservation,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerRegisterObservation,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothModemLpTimerPendingObservation, BluetoothModemLpTimerStoppedInterruptFault,
    BluetoothModemLpTimerStoppedInterruptObservation, BluetoothNrtInterruptAcknowledged,
    BluetoothNrtInterruptFault, BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
    BluetoothRawTickDeltaProjection, BluetoothScanStartPublished,
    BluetoothSchedulerBusyObservation, BluetoothSchedulerCancellationDisposition,
    BluetoothSchedulerCancellationIndexed, BluetoothSchedulerCancellationReleased,
    BluetoothSchedulerCancellationRequested, BluetoothSchedulerCancellationSourceAcknowledged,
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerExecutionModifyPublished, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareListsCleared,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerInsertionCommand,
    BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyTaskObservation,
    BluetoothSchedulerReferenceCleared, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerSkipCleared,
    BluetoothSchedulerSkipDisposition, BluetoothSchedulerSkipPublished,
    BluetoothSchedulerSkipResult, BluetoothSchedulerSoftwareListRemovalIdle,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerStopped,
    BluetoothSchedulerWorkObservation,
};
use registers::{
    BluetoothMemoryListPointerImage, BluetoothSchedulerExecutionLockRequest,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerRunEventPublished, BluetoothSchedulerSkipRequest,
    BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerStoppedHeadRetirement,
};

/// The Bluetooth parts of the PAC root: the controller with modem ETM
/// channel two, the modem low-power timer with modem ETM channels four
/// through seven, and the interrupt bank.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothPartition {
    /// The controller partition and modem ETM channel two.
    pub controller: BluetoothControllerParts,
    /// The modem low-power timer and modem ETM channels four through seven.
    pub modem_lp_timer: BluetoothModemLpTimerParts,
    /// The interrupt bank and the scheduler's diagnostic pair.
    pub interrupts: oer_esp32s31_pac::ownership::BluetoothInterruptPeripherals,
}

/// The Bluetooth partition split into its task registers, modem low-power
/// timer and inactive interrupt bank.
struct Partition {
    task: BluetoothTaskRegisters,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: RegistersBluetoothInterruptSetup,
}

/// The type naming the interrupt sources of the controller's bank: the
/// primary source 124 and the NRT source 133, which one handler samples and
/// acknowledges through the bank (`InterruptSources`).
pub type BluetoothInterruptBankSources = oer_esp32s31_pac::bt_mac::BluetoothInterruptBank;

/// The type naming the modem low-power timer's interrupt source 127
/// (`InterruptSource`).
pub type ModemLpTimerSource = oer_esp32s31_pac::modem_pwr::BtdmRuntimeControl;

/// Unpowered Bluetooth client holding its partition.
///
/// Construction splits the partition without touching MMIO. It proves
/// neither power nor clocks, common-PHY, BTBB, IRQ or Controller readiness.
#[must_use = "the cold Bluetooth client retains its radio partition"]
pub struct ColdOwner {
    partition: Partition,
}

/// Read-only LP cause capability held permanently by the interrupt handler.
/// Copies read the side-effect-free status; none can acknowledge or arm it.
#[derive(Clone, Copy)]
pub struct ModemLpTimerStatus(oer_esp32s31_pac::modem_pwr::BtdmLpTimerStatus);

impl ModemLpTimerStatus {
    /// The complete LP cause word, without acknowledgement or timer mutation.
    pub fn read(&self) -> u32 {
        self.0.read_bluetooth_modem_lp_status()
    }
}

impl ColdOwner {
    /// Retain observation authority independently of the timer's lifecycle.
    pub fn timer_status(&self) -> ModemLpTimerStatus {
        ModemLpTimerStatus(self.partition.modem_lp_timer.status)
    }

    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: BluetoothPartition) -> Self {
        let BluetoothPartition {
            controller,
            modem_lp_timer,
            interrupts,
        } = partition;
        Self {
            partition: Partition {
                task: BluetoothTaskRegisters::new(controller),
                modem_lp_timer: BluetoothModemLpTimerRegisters::new(modem_lp_timer),
                interrupts: RegistersBluetoothInterruptSetup::new(interrupts),
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
        BluetoothPartition {
            controller: task.into_parts(),
            modem_lp_timer: modem_lp_timer.into_parts(),
            interrupts: interrupts.into_partition(),
        }
    }

    /// Enter common radio power as the Bluetooth client.
    ///
    /// Only the first client runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power or the first client's sequence failed a
    /// read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        clocks: &impl PlatformClockProvider<Backend = B>,
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
    pub fn power_down<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
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
    pub fn enable_clocks<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        platform: &impl PlatformClockProvider<Backend = B>,
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
    pub fn disable_clocks<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
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
                epoch: ControllerEpoch::next(),
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
    epoch: ControllerEpoch,
}

/// One task owner's span, from its separation out of the clocked owner to its
/// return: the Controller's lists hold what this span published, and a reset
/// clears them only between spans. A publication proof of another span is
/// stale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ControllerEpoch(u32);

impl ControllerEpoch {
    fn next() -> Self {
        static EPOCHS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        Self(EPOCHS.fetch_add(1, core::sync::atomic::Ordering::Relaxed))
    }
}

impl TaskOwner {
    /// Reset the stopped Controller and await the timer its handler gives
    /// up after the reset.
    ///
    /// The caller has released Controller output, drained the source-127
    /// software work, had the timer's handler stop the counter and both
    /// measured delivery lanes and left the shared PHY domain and BTBB baseband. The Controller retains every
    /// published memory owner until the reunion.
    #[doc(hidden)]
    pub fn shut_down<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        output: InterruptOutputReleasedOwner,
        stopped: ModemLpTimerStopped,
    ) -> Result<BluetoothControllerResetTimerPending, BluetoothShutdownFailure> {
        shutdown::shut_down(self, radio, output.registers, stopped)
    }

    /// Admit the release of the controller output: no controller-time
    /// request is in flight. Then retain the task bank's preflight observation;
    /// writes nothing.
    ///
    /// # Errors
    ///
    /// [`BluetoothControllerOutputReleaseError::ControllerTimePending`].
    pub fn admit_output_release(
        &mut self,
    ) -> Result<BluetoothOutputReleaseAdmitted, BluetoothControllerOutputReleaseError> {
        if self.time_latch.in_flight() {
            return Err(BluetoothControllerOutputReleaseError::ControllerTimePending);
        }
        Ok(BluetoothOutputReleaseAdmitted {
            task_enable: self.interrupt_enable_image(),
        })
    }

    /// After the handler's idle sample, check that no scheduler list names
    /// hardware-visible memory. Writes nothing.
    ///
    /// # Errors
    ///
    /// [`BluetoothControllerOutputReleaseError::PublishedHead`]; the list is
    /// left unchanged.
    pub fn confirm_scheduler_heads_absent(
        &mut self,
        _sampled: BluetoothOutputIdleSampled,
    ) -> Result<BluetoothSchedulerHeadsAbsent, BluetoothControllerOutputReleaseError> {
        self.registers.confirm_scheduler_heads_absent()?;
        Ok(BluetoothSchedulerHeadsAbsent {
            task_enable: self.interrupt_enable_image(),
        })
    }

    /// After the handler masked the dynamic scheduler sources, disable
    /// scheduler RUN and fence.
    pub fn disable_scheduler_run(
        &mut self,
        _masked: BluetoothDynamicSourcesMasked,
    ) -> BluetoothSchedulerRunDisabled {
        self.registers.disable_scheduler_run();
        let task_enable = self.interrupt_enable_image();
        BluetoothSchedulerRunDisabled { task_enable }
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
            epoch: _,
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
    /// clocks. Only the first holder runs `bt_bb_v2_init_cmplx(1)`, with
    /// `gain`'s byte.
    ///
    /// # Errors
    ///
    /// As [`RadioDomain::btbb_acquire`].
    pub fn acquire_btbb<B: PlatformClockBackend, T>(
        &self,
        radio: &mut RadioDomain<B, T>,
        gain: &crate::domain::BtbbGain,
    ) -> Result<BtbbAcquired, BtbbError> {
        radio.btbb_acquire(&self.registers, gain)
    }

    /// Leave the shared BTBB baseband.
    ///
    /// # Errors
    ///
    /// Bluetooth does not hold BTBB.
    pub fn release_btbb<B: PlatformClockBackend, T>(
        &self,
        radio: &mut RadioDomain<B, T>,
    ) -> Result<(), BtbbError> {
        radio.btbb_release(&self.registers)
    }

    /// Execute the complete reviewed BLE base-stack task-enable hardware transaction.
    ///
    /// The two objects the transaction points the hardware at come from
    /// `storage`; the caller runs it with the common-PHY and BTBB membership,
    /// the initialized controller software and an inactive IRQ route, orders
    /// of the Controller's lifecycle.
    #[doc(hidden)]
    pub(crate) fn enable_ble_base_stack_hardware<B: PlatformClockBackend, T>(
        &mut self,
        radio: &mut RadioDomain<B, T>,
        storage: &BlePhyHardwareObjects,
    ) {
        self.reunitable = false;
        self.registers.enable_ble_base_stack_hardware(
            &mut radio.registers_mut().shared_radio.bt_v32_baseband,
            storage.inputs,
        );
    }

    /// Execute the complete reviewed 49-operation controller HAL-init body
    /// at the upper controller lifecycle's verified transition.
    ///
    /// The SRAM prefix it publishes is the fixed `0x2f` of its register
    /// image, so every compressed address the Controller follows resolves
    /// into that SRAM window. The caller does not infer scheduler, PHY, BTBB,
    /// Link-Layer or HCI readiness from return.
    #[doc(hidden)]
    pub fn initialize_controller_hal_transaction(
        &mut self,
        config: BluetoothControllerHalInitConfig,
    ) {
        self.reunitable = false;
        self.registers.initialize_controller_hal(config);
    }

    /// Apply the exact modem low-power timer register prefix before source 127
    /// starts. Preparation arms the independent rollover lane with the
    /// counter stopped, after sampling and clearing both inherited pending
    /// bytes. The returned observation exposes the removed stale state.
    ///
    /// This extracts only the generated timer partition. Controller task
    /// ownership remains available for scheduler and Link-Layer progress while
    /// source 127 retains exclusive timer-register authority.
    ///
    /// The caller runs it with initialized controller and timer software and
    /// before source 127 can reach the CPU: an order of the Controller's
    /// lifecycle, which publishes no memory to the hardware.
    #[doc(hidden)]
    pub fn prepare_modem_lp_timer_registers(
        &mut self,
    ) -> Result<
        (
            ModemLpTimerRegistersPreparedOwner,
            BluetoothModemLpTimerPendingObservation,
        ),
        BluetoothModemLpTimerOwnerError,
    > {
        self.reunitable = false;
        // Extract the partition before the first MMIO effect: cancellation or
        // panic can only lose timer authority fail-stop.
        let mut timer = self
            .modem_lp_timer
            .take()
            .ok_or(BluetoothModemLpTimerOwnerError::OwnerSeparated)?;
        let stale = timer.prepare_registers();
        Ok((ModemLpTimerRegistersPreparedOwner { timer }, stale))
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
    /// The caller runs it with the matching initialized controller software,
    /// after the timer-register prefix and before source 127 can reach the
    /// CPU: an order of the Controller's lifecycle, which publishes no memory
    /// to the hardware.
    #[doc(hidden)]
    pub fn initialize_low_power_hardware(
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

    /// The unique started timer as the source-127 handler holds it.
    ///
    /// This transition performs no MMIO and exposes no raw register owner.
    /// The timer's handler starts the timer and keeps the returned owner
    /// until the Controller reset. Software work receives only a time snapshot.
    pub fn stage_for_interrupt(self) -> ModemLpTimerInterruptReadyOwner {
        ModemLpTimerInterruptReadyOwner {
            timer: self.timer,
            epoch: BluetoothModemLpTimerEpoch::new(),
        }
    }
}

/// Opaque HAL owner staged for the source-127 hard handler.
#[must_use = "the modem LP-timer interrupt owner must remain in stable ISR storage"]
pub struct ModemLpTimerInterruptReadyOwner {
    timer: BluetoothModemLpTimerRegisters,
    epoch: BluetoothModemLpTimerEpoch,
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
    epoch: BluetoothModemLpTimerEpoch,
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
        let mut epoch = self.epoch;
        match timer.take_software_work(&mut epoch) {
            None => ModemLpTimerHandlerRegisterStep::Rearmed(ModemLpTimerInterruptReadyOwner {
                timer,
                epoch,
            }),
            Some(work) => ModemLpTimerHandlerRegisterStep::SoftwarePending {
                owner: ModemLpTimerSoftwarePendingOwner {
                    timer,
                    epoch,
                    deferred_dispatch: None,
                    dispatch_requested: work.dispatch_requested(),
                },
                work,
            },
        }
    }
}

/// Result of the common handler's bounded register-acknowledgement phase.
#[must_use = "retain the register owner and hand software work to the task"]
pub enum ModemLpTimerHandlerRegisterStep {
    /// No software work was requested and the interrupt owner is ready again.
    Rearmed(ModemLpTimerInterruptReadyOwner),
    /// Keep the register owner in the handler and publish the value-only work.
    SoftwarePending {
        /// Register authority retained by the handler until its plan arrives.
        owner: ModemLpTimerSoftwarePendingOwner,
        /// Snapshot for the task's bounded queue processing.
        work: ModemLpTimerWork,
    },
}

/// Value-only source-127 work. It grants no register access.
///
/// ```compile_fail
/// use oer_esp32s31_hal::bluetooth::{ModemLpTimerWork, BluetoothModemLpTimerEpoch};
/// fn task(mut work: ModemLpTimerWork, epoch: &mut BluetoothModemLpTimerEpoch) {
///     work.sample_counter(epoch);
/// }
/// ```
#[must_use = "complete software work and return its unarmed plan"]
pub struct ModemLpTimerWork {
    now: BluetoothModemLpTimerInstant,
    epoch: BluetoothModemLpTimerEpoch,
    dispatch_requested: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl ModemLpTimerWork {
    #[doc(hidden)]
    pub const fn for_host_test(
        now: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
        dispatch_requested: bool,
    ) -> Self {
        Self {
            now,
            epoch,
            dispatch_requested,
        }
    }
}

impl ModemLpTimerWork {
    /// Handler-sampled positional time, including acknowledged rollovers.
    pub const fn instant(&self) -> BluetoothModemLpTimerInstant {
        self.now
    }

    /// The handler's epoch at the snapshot; software retains only a copy.
    pub const fn epoch(&self) -> BluetoothModemLpTimerEpoch {
        self.epoch
    }

    /// Whether the acknowledged state requested queue dispatch.
    pub const fn dispatch_requested(&self) -> bool {
        self.dispatch_requested
    }

    /// Finish queue work without touching or arming hardware.
    pub fn plan(self, deadline: Option<BluetoothModemLpTimerInstant>) -> ModemLpTimerPlan {
        ModemLpTimerPlan { deadline }
    }
}

/// An unarmed queue decision. Only the handler's retained owner applies it.
#[must_use = "submit the plan to the timer handler"]
pub struct ModemLpTimerPlan {
    deadline: Option<BluetoothModemLpTimerInstant>,
}

impl ModemLpTimerPlan {
    /// Selected software deadline, or an empty queue. This is not armed state.
    pub const fn deadline(&self) -> Option<BluetoothModemLpTimerInstant> {
        self.deadline
    }
}

/// Register owner retained by the handler while P2 calculates a plan.
#[must_use = "retain this owner in the handler until the software plan arrives"]
pub struct ModemLpTimerSoftwarePendingOwner {
    timer: BluetoothModemLpTimerRegisters,
    epoch: BluetoothModemLpTimerEpoch,
    deferred_dispatch: Option<bool>,
    dispatch_requested: bool,
}

impl ModemLpTimerSoftwarePendingOwner {
    /// Stop only rollover work. The retired epoch needs no further deadline
    /// projection; its outstanding task snapshot/epoch shift is discarded.
    /// Dispatch requested by that snapshot or a later entry refuses without
    /// changing registers or discarding any software work.
    #[allow(
        clippy::result_large_err,
        reason = "refusal retains the register owner"
    )]
    pub fn stop_for_shutdown(
        self,
    ) -> Result<(ModemLpTimerStoppedOwner, ModemLpTimerStopped), Self> {
        if self.dispatch_requested || self.deferred_dispatch == Some(true) {
            return Err(self);
        }
        Ok(ModemLpTimerInterruptReadyOwner {
            timer: self.timer,
            epoch: self.epoch,
        }
        .stop_for_shutdown())
    }

    /// Acknowledge a new interrupt while P2 owns the earlier snapshot. Retain
    /// its software consequence here without overwriting the in-flight work.
    /// Rollover acknowledgement advances only this handler-owned epoch.
    pub fn observe_interrupt(&mut self) {
        self.timer
            .retain_interrupt(&mut self.epoch, &mut self.deferred_dispatch);
    }

    /// Apply the unarmed plan while holding the timer's sole register authority.
    ///
    /// A deadline uses the sourced disable/sample/program/bit-18 transaction.
    /// Its rollover projection does not clear the rollover byte: the next
    /// handler register phase acknowledges it and advances the retained epoch
    /// exactly once. Pending arrivals stay in hardware or in this owner's deferred work.
    /// The empty queue disables compare only; it proves no all-cause stop.
    pub fn apply_plan(self, plan: ModemLpTimerPlan) -> ModemLpTimerHandlerRegisterStep {
        let mut timer = self.timer;
        let mut epoch = self.epoch;
        match timer.apply_plan(plan, &mut epoch, self.deferred_dispatch) {
            Some(work) => ModemLpTimerHandlerRegisterStep::SoftwarePending {
                owner: Self {
                    timer,
                    epoch,
                    deferred_dispatch: None,
                    dispatch_requested: work.dispatch_requested(),
                },
                work,
            },
            None => ModemLpTimerHandlerRegisterStep::Rearmed(ModemLpTimerInterruptReadyOwner {
                timer,
                epoch,
            }),
        }
    }
}

/// The counter is stopped and both measured delivery lanes are disabled.
/// Both pending words were cleared after retaining their stop-time observation.
#[derive(Debug)]
#[must_use = "the stopped timer admits the Controller reset"]
pub struct ModemLpTimerStopped {
    pending: BluetoothModemLpTimerPendingObservation,
}

impl ModemLpTimerStopped {
    /// Pending bytes sampled after counter stop, before both words were cleared.
    pub const fn pending_observation(&self) -> BluetoothModemLpTimerPendingObservation {
        self.pending
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ModemLpTimerStopped {
    #[doc(hidden)]
    pub fn for_host_test() -> Self {
        Self {
            pending: BluetoothModemLpTimerPendingObservation::default(),
        }
    }
}

/// Register authority retained after stop, until the Controller reset.
///
/// A stopped owner cannot resume the running interrupt path:
/// ```compile_fail
/// use oer_esp32s31_hal::bluetooth::ModemLpTimerStoppedOwner;
/// fn rearm(timer: ModemLpTimerStoppedOwner) { let _ = timer.step(); }
/// ```
#[must_use = "the stopped owner stays with the handler until reset"]
pub struct ModemLpTimerStoppedOwner {
    timer: BluetoothModemLpTimerRegisters,
    epoch: BluetoothModemLpTimerEpoch,
}

impl ModemLpTimerStoppedOwner {
    /// Acknowledge rollover without sampling time or rearming delivery.
    /// Invalid software pending or unknown nonzero status remains for the
    /// gate's existing fault path.
    pub fn acknowledge_interrupt(
        &mut self,
    ) -> Result<
        BluetoothModemLpTimerStoppedInterruptObservation,
        BluetoothModemLpTimerStoppedInterruptFault,
    > {
        self.timer.acknowledge_stopped_interrupt(&mut self.epoch)
    }
}

impl ModemLpTimerInterruptReadyOwner {
    /// Stop both measured delivery lanes and the counter after the task drained.
    /// The handler keeps the stopped register owner until Controller reset.
    pub fn stop_for_shutdown(mut self) -> (ModemLpTimerStoppedOwner, ModemLpTimerStopped) {
        let pending = self.timer.stop_for_shutdown();
        (
            ModemLpTimerStoppedOwner {
                timer: self.timer,
                epoch: self.epoch,
            },
            ModemLpTimerStopped { pending },
        )
    }

    /// Perform one finite source-127 register classification.
    ///
    /// The unique ISR-staged owner is the authority for exactly one register
    /// pass. The method never waits, loops, allocates or invokes an RTOS
    /// service.
    pub fn step(self) -> ModemLpTimerInterruptStep {
        let mut timer = self.timer;
        match timer.classify_interrupt() {
            None => ModemLpTimerInterruptStep::Spurious(ModemLpTimerInterruptReadyOwner {
                timer,
                epoch: self.epoch,
            }),
            Some(observation) => {
                ModemLpTimerInterruptStep::HandlerPending(ModemLpTimerHandlerPendingOwner {
                    timer,
                    epoch: self.epoch,
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
    registers: RegistersBluetoothInterruptSetup,
    reunitable: bool,
}

impl InterruptSetupOwner {
    /// Execute the reviewed baseline clear/enable/output preparation.
    ///
    /// The bank's interrupt handler runs it when the task opens the epoch,
    /// after the completed controller HAL-init and with quiescent dynamic
    /// sources: an order of the Controller's lifecycle, which publishes no
    /// memory to the hardware. The handler holds the prepared bank, so no
    /// armed source reaches a line whose handler lacks it.
    pub fn prepare_controller_output(self) -> InterruptOutputPreparedOwner {
        InterruptOutputPreparedOwner {
            registers: self.registers.prepare_controller_output(),
        }
    }
}

/// Controller IRQ output prepared in the bank's handler, not yet staged.
#[must_use = "the prepared Bluetooth output must be staged or released"]
pub struct InterruptOutputPreparedOwner {
    registers: registers::BluetoothInterruptOutputPrepared,
}

impl InterruptOutputPreparedOwner {
    /// The prepared bank as its handler holds it for the epoch.
    pub fn stage_for_handler(self) -> InterruptRegistersOwner {
        InterruptRegistersOwner {
            registers: self.registers.stage_for_handler(),
            #[cfg(feature = "bluetooth-engine")]
            diagnostic_selection: DiagnosticSelection::None,
            fault_acknowledged: false,
            released: false,
        }
    }

    /// Execute the reviewed controller-output release transaction on a bank
    /// that never reached its handler's epoch.
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

/// The interrupt bank as the handler of sources 124 and 133 holds it for
/// one epoch: from the prepared output to its release.
///
/// The bank shares one line with the other Bluetooth sources, so every
/// capture first samples and writes nothing when its status is clear. A
/// primary fault the handler acknowledges stays recorded and refuses the
/// output release; once the output is released, no scheduler-run group is
/// enabled again.
#[must_use = "the Bluetooth interrupt bank leaves its handler only released"]
pub struct InterruptRegistersOwner {
    registers: BluetoothInterruptRegisters,
    #[cfg(feature = "bluetooth-engine")]
    diagnostic_selection: DiagnosticSelection,
    fault_acknowledged: bool,
    released: bool,
}

#[cfg(feature = "bluetooth-engine")]
enum DiagnosticSelection {
    None,
    Progress,
    Settle,
}

impl InterruptRegistersOwner {
    /// Prepare the dynamic interrupt groups required immediately before a
    /// scheduler run publication; `None` once the output is released.
    pub fn prepare_scheduler_run_interrupts(
        &mut self,
    ) -> Option<BluetoothSchedulerRunInterruptsPrepared> {
        if self.released {
            return None;
        }
        Some(self.registers.prepare_scheduler_run_interrupts())
    }

    /// Diagnostic NRT causes before any acknowledgement, without release authority.
    pub fn nrt_status(&self) -> [u32; 2] {
        self.registers.nrt_status()
    }

    /// Capture and acknowledge one complete opaque NRT source-133 epoch;
    /// `Ok(None)`, writing nothing, when both snapshots are clear.
    ///
    /// This preserves the sample/sample/acknowledge/acknowledge order. It
    /// deliberately performs no feature dispatch and publishes no task wake.
    /// An NRT sample containing a primary fault or overflow clear lane returns a
    /// fault retaining both snapshots, without acknowledging either bank.
    pub fn capture_nrt_and_acknowledge(
        &mut self,
    ) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault> {
        self.registers.capture_nrt_and_acknowledge()
    }

    /// Capture, acknowledge and retain one complete primary source-124
    /// epoch; `None`, writing nothing, when both status words are clear. A
    /// fault it acknowledges refuses the output release of this epoch.
    pub fn capture_primary_and_acknowledge(&mut self) -> Option<BluetoothPrimaryInterruptEpoch> {
        let epoch = self.registers.capture_primary_and_acknowledge()?;
        self.fault_acknowledged |= epoch.fault_sources().is_fault();
        Some(epoch)
    }

    /// Sample scheduler BUSY through the diagnostic pair for the bank-one
    /// source-3 reference gate, within `settle`.
    pub fn capture_scheduler_reference_gate<C: oer_time::Clock + ?Sized>(
        &mut self,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, Unsettled> {
        let result = settle::settle(settle, || self.registers.capture_scheduler_reference_gate());
        #[cfg(feature = "bluetooth-engine")]
        self.restore_diagnostic_selection();
        result
    }

    /// Clear the scheduler reference selected by the source-124 gate.
    ///
    /// The returned affine token proves the ordered write and device fence.
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
    /// scheduler decision, within `settle`.
    pub fn capture_scheduler_busy<C: oer_time::Clock + ?Sized>(
        &mut self,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothSchedulerBusyObservation, Unsettled> {
        let result = settle::settle(settle, || self.registers.capture_scheduler_busy());
        #[cfg(feature = "bluetooth-engine")]
        self.restore_diagnostic_selection();
        result
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
    /// registers, within `settle`.
    pub fn capture_scheduler_lock_modify_interrupt<C: oer_time::Clock + ?Sized>(
        &mut self,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothSchedulerLockModifyInterruptObservation, Unsettled> {
        let result = settle::settle(settle, || {
            self.registers.capture_scheduler_lock_modify_interrupt()
        });
        #[cfg(feature = "bluetooth-engine")]
        self.restore_diagnostic_selection();
        result
    }

    #[cfg(feature = "bluetooth-engine")]
    fn restore_diagnostic_selection(&mut self) {
        match self.diagnostic_selection {
            DiagnosticSelection::None => {}
            DiagnosticSelection::Progress => {
                self.registers.select_scheduler_execution_modify_progress()
            }
            DiagnosticSelection::Settle => {
                self.registers.select_scheduler_execution_modify_settle()
            }
        }
    }

    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn mask_scheduler_run_interrupts(&mut self) {
        self.registers.mask_scheduler_run_interrupts();
    }
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn select_scheduler_execution_modify_progress(&mut self) {
        self.diagnostic_selection = DiagnosticSelection::Progress;
        self.restore_diagnostic_selection();
    }
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn select_scheduler_execution_modify_settle(&mut self) {
        self.diagnostic_selection = DiagnosticSelection::Settle;
        self.restore_diagnostic_selection();
    }
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn scheduler_execution_modify_repeats<C: oer_time::Clock + ?Sized>(
        &mut self,
        budget: Settle<'_, C>,
    ) -> Result<bool, Unsettled> {
        let result = settle::settle(budget, || {
            self.registers.scheduler_execution_modify_repeats()
        });
        self.diagnostic_selection = DiagnosticSelection::None;
        result
    }
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn scheduler_execution_modify_settled<C: oer_time::Clock + ?Sized>(
        &mut self,
        budget: Settle<'_, C>,
    ) -> Result<bool, Unsettled> {
        let result = settle::settle(budget, || {
            self.registers.scheduler_execution_modify_settled()
        });
        self.diagnostic_selection = DiagnosticSelection::None;
        result
    }

    /// Admit the release on an idle BUSY sample, taken within `settle`,
    /// with no primary fault pending or acknowledged during the epoch.
    /// Writes nothing.
    ///
    /// # Errors
    ///
    /// The sample did not settle, the scheduler is busy, or a fault is
    /// pending or was acknowledged.
    pub fn sample_idle_output<C: oer_time::Clock + ?Sized>(
        &mut self,
        _admitted: &BluetoothOutputReleaseAdmitted,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothOutputIdleSampled, BluetoothControllerOutputReleaseError> {
        let busy = self.release_busy_sample(settle)?;
        self.registers
            .sample_idle_output(busy, self.fault_acknowledged, _admitted.task_enable)?;
        Ok(BluetoothOutputIdleSampled { _private: () })
    }

    /// Acknowledge admitted sampled NRT, check visible faults, then mask the
    /// sourced terminal-cleanup sources and fence, once the task found no list
    /// head published. A visible fault retains
    /// its observations for quarantine; no fault lane is acknowledged.
    pub fn mask_output_dynamic_sources(
        &mut self,
        _heads: &BluetoothSchedulerHeadsAbsent,
    ) -> Result<BluetoothDynamicSourcesMasked, BluetoothControllerOutputReleaseError> {
        self.registers
            .mask_output_dynamic_sources(self.fault_acknowledged, _heads.task_enable)?;
        Ok(BluetoothDynamicSourcesMasked { _private: () })
    }

    /// Discard the fixed non-fault primary/NRT subset under the mask and release output,
    /// once the task disabled RUN, on a fresh idle BUSY sample within
    /// `settle` with no visible fault pending or acknowledged. Masked STATUS
    /// does not prove pending is absent. The runtime NRT transaction precedes
    /// final release confirmation; a refused pair or residual visible status
    /// returns `ReleaseNotQuiet`.
    ///
    /// # Errors
    ///
    /// The sample did not settle, the scheduler is busy, or a fault is
    /// pending or was acknowledged; the output stays prepared.
    pub fn release_idle_output<C: oer_time::Clock + ?Sized>(
        &mut self,
        _run: &BluetoothSchedulerRunDisabled,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothOutputReleased, BluetoothControllerOutputReleaseError> {
        let busy = self.release_busy_sample(settle)?;
        self.registers
            .release_idle_output(busy, self.fault_acknowledged, _run.task_enable)?;
        self.released = true;
        Ok(BluetoothOutputReleased { _private: () })
    }

    /// Whether the output of this epoch is released.
    pub const fn output_released(&self) -> bool {
        self.released
    }

    /// The released bank, leaving its handler. Performs no MMIO.
    ///
    /// # Errors
    ///
    /// The output of this owner is not released; the owner comes back.
    pub fn into_released(
        self,
        _released: BluetoothOutputReleased,
    ) -> Result<InterruptOutputReleasedOwner, Self> {
        if !self.released {
            return Err(self);
        }
        Ok(InterruptOutputReleasedOwner {
            registers: self.registers.into_released(),
        })
    }

    fn release_busy_sample<C: oer_time::Clock + ?Sized>(
        &mut self,
        settle: Settle<'_, C>,
    ) -> Result<BluetoothSchedulerBusyObservation, BluetoothControllerOutputReleaseError> {
        self.capture_scheduler_busy(settle)
            .map_err(|Unsettled| BluetoothControllerOutputReleaseError::SchedulerBusyUnsettled)
    }
}

/// The task admitted the output release: no controller-time request is in
/// flight.
#[derive(Debug)]
#[must_use = "the admission feeds the handler's idle sample"]
pub struct BluetoothOutputReleaseAdmitted {
    task_enable: u32,
}

/// The handler sampled an idle scheduler without a fault.
#[derive(Debug)]
#[must_use = "the idle sample feeds the task's list-head check"]
pub struct BluetoothOutputIdleSampled {
    _private: (),
}

/// The task found no scheduler list head published.
#[derive(Debug)]
#[must_use = "the empty lists admit the handler's mask"]
pub struct BluetoothSchedulerHeadsAbsent {
    task_enable: u32,
}

/// The handler masked the dynamic scheduler sources and fenced.
#[derive(Debug)]
#[must_use = "the masked sources admit the task's RUN disable"]
pub struct BluetoothDynamicSourcesMasked {
    _private: (),
}

/// The task disabled scheduler RUN and fenced.
#[derive(Debug)]
#[must_use = "the disabled RUN admits the handler's release"]
pub struct BluetoothSchedulerRunDisabled {
    task_enable: u32,
}

/// The handler released the controller output of its epoch.
#[derive(Debug)]
#[must_use = "the released output lets the bank leave its handler"]
pub struct BluetoothOutputReleased {
    _private: (),
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothOutputReleaseAdmitted {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { task_enable: 0 }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothOutputIdleSampled {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothSchedulerHeadsAbsent {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { task_enable: 0 }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothDynamicSourcesMasked {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothSchedulerRunDisabled {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { task_enable: 0 }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BluetoothOutputReleased {
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

/// The released interrupt bank after its handler's epoch: no active IRQ or
/// setup conversion is exposed; it is retained through the Controller reset.
#[must_use = "released output must remain paired with its stopped Controller"]
pub struct InterruptOutputReleasedOwner {
    registers: RegistersBluetoothInterruptSetup,
}

/// Exclusive finite borrow of the Bluetooth controller task-side registers.
///
/// This type deliberately exposes neither `Deref`, a raw register accessor nor a
/// constructor. New operations belong here only after their register transaction
/// and lifecycle prerequisites are independently bounded.
pub struct ControllerHal<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
    time_latch: &'registers mut ControllerTimeLatch,
    epoch: ControllerEpoch,
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
    head: crate::layouts::DmaRef<crate::layouts::BleBufferHeader>,
    scanning: bool,
    epoch: ControllerEpoch,
}

/// Why the scanner start was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStartError {
    /// The publication is not the scanning RX list's.
    NotScanningList,
    /// Another Controller epoch published the list.
    StaleList,
}

/// The two objects of the BLE PHY engine that the base-stack enable points
/// the hardware at.
pub(crate) struct BlePhyHardwareObjects {
    inputs: BluetoothPhyRegisterInitInputs,
}

impl BlePhyHardwareObjects {
    /// Bind the PHY profile's typed references to retained volatile layouts.
    /// Their `DmaRef`s carry the complete placement and alignment proof.
    pub(crate) const fn bind(inputs: BluetoothPhyRegisterInitInputs) -> Self {
        Self { inputs }
    }
}

/// The disabled-CTE descriptor the direction-finding baseline points the
/// hardware at.
pub(crate) struct DirectionFindingDescriptor {
    address: crate::layouts::DmaRef<crate::layouts::BleDirectionFindingDescriptor>,
}

impl DirectionFindingDescriptor {
    /// Bind the workspace projection's typed descriptor reference, which
    /// already proves its complete extent, alignment and retention.
    pub(crate) const fn bind(
        address: crate::layouts::DmaRef<crate::layouts::BleDirectionFindingDescriptor>,
    ) -> Self {
        Self { address }
    }
}

/// The device table hardware filters against: its first entry and count.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DeviceTablePublication {
    first_entry: crate::layouts::DmaRef<crate::layouts::BleDeviceTable>,
    count: u8,
}

impl DeviceTablePublication {
    pub(crate) const CAPACITY: usize =
        crate::layouts::BleDeviceTable::SIZE / crate::layouts::BleDeviceTableEntry::SIZE;

    /// Bind a complete retained table to a count admitted within its extent.
    /// The full-table `DmaRef` proves placement and alignment; the bounded
    /// count includes zero and the table's full capacity.
    pub(crate) const fn new(
        first_entry: crate::layouts::DmaRef<crate::layouts::BleDeviceTable>,
        count: oer_mmio::Index<{ Self::CAPACITY + 1 }>,
    ) -> Self {
        const { assert!(DeviceTablePublication::CAPACITY <= u8::MAX as usize) };
        let count = count.get() as u8;
        Self { first_entry, count }
    }

    /// Address of the first entry.
    pub(crate) const fn first_entry(
        &self,
    ) -> crate::layouts::DmaRef<crate::layouts::BleDeviceTable> {
        self.first_entry
    }

    /// Number of entries.
    pub const fn count(&self) -> u8 {
        self.count
    }
}

/// The first header of one receive chain and the list it belongs to.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct RxChainHead {
    selector: BluetoothMemoryListSelector,
    head: crate::layouts::DmaRef<crate::layouts::BleBufferHeader>,
    scanning: bool,
}

impl RxChainHead {
    /// Bind a retained volatile header to its receive-list selection.
    /// The `DmaRef` proves placement and alignment; the receive-chain owner
    /// initializes and retains the linked graph and chooses its list.
    pub(crate) const fn new(
        selector: BluetoothMemoryListSelector,
        head: crate::layouts::DmaRef<crate::layouts::BleBufferHeader>,
        scanning: bool,
    ) -> Self {
        Self {
            selector,
            head,
            scanning,
        }
    }
}

/// HAL ownership of the controller-global disabled-CTE publication.
///
/// The register-level proof remains private, so upper Controller layers can
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
    pub(crate) const fn head(&self) -> crate::layouts::DmaRef<crate::layouts::BleBufferHeader> {
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
    head: crate::layouts::DmaRef<crate::layouts::BleBufferHeader>,
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

    fn reset_initial_control(&mut self) {
        self.registers
            .reset_memory_list_initial_control(self.selector);
    }

    fn publish_current_head(&mut self) {
        self.registers.program_memory_list_pointer(
            self.selector,
            BluetoothMemoryListSlot::CurrentRx,
            BluetoothMemoryListPointerImage::Address(self.head),
        );
    }

    fn clear_next_head(&mut self) {
        self.registers.program_memory_list_pointer(
            self.selector,
            BluetoothMemoryListSlot::NextRx,
            BluetoothMemoryListPointerImage::Zero,
        );
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
    /// The descriptor comes from `descriptor`; the caller runs it in the
    /// powered Controller epoch, an order of its lifecycle.
    #[doc(hidden)]
    pub(crate) fn prepare_direction_finding_disabled_baseline(
        &mut self,
        descriptor: &DirectionFindingDescriptor,
    ) -> DirectionFindingDisabledBaselineOwner {
        let prepared = self
            .registers
            .prepare_direction_finding_disabled_baseline(descriptor.address);
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
    /// The head comes from `chain`; the caller runs it in the powered
    /// Controller epoch with no active RUN and serializes the list's task and
    /// interrupt access, orders of the Controller's lifecycle.
    #[doc(hidden)]
    pub(crate) fn publish_rx_memory_list_initial_head(
        &mut self,
        chain: &RxChainHead,
    ) -> RxMemoryListPublished {
        let RxChainHead {
            selector,
            head,
            scanning,
        } = *chain;
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::ControllerDefault,
        );
        RxMemoryListPublished {
            selector,
            head,
            scanning,
            epoch: self.epoch,
        }
    }

    /// Publish RX for a connection whose control PDUs are owned by software.
    ///
    /// Configure opcode inspection before publishing the RX head. Software CCM
    /// ciphertext must reach the authenticating LL decoder even when its first
    /// byte matches a hardware abort opcode. Repeating this transaction also
    /// restores the policy after PHY maintenance reinitializes BLE registers.
    ///
    /// The head comes from `chain`; the caller runs it in the powered
    /// Controller epoch with no active RUN and serializes the list's task and
    /// interrupt access, orders of the Controller's lifecycle.
    #[doc(hidden)]
    pub(crate) fn publish_software_connection_rx_memory_list_initial_head(
        &mut self,
        chain: &RxChainHead,
    ) -> RxMemoryListPublished {
        let RxChainHead {
            selector,
            head,
            scanning,
        } = *chain;
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::SoftwareConnection,
        );
        RxMemoryListPublished {
            selector,
            head,
            scanning,
            epoch: self.epoch,
        }
    }

    /// Publish one complete reviewed scanner command transaction.
    ///
    /// The engine starts only with the scanning list publication of the same
    /// Controller epoch. Its retained arena holds the scanner graph, and the
    /// exclusive register loan serializes these backoff writes.
    ///
    /// # Errors
    ///
    /// `scanning` is not the scanning list's publication, or another epoch
    /// published it; nothing is written.
    #[doc(hidden)]
    pub fn publish_scan_start(
        &mut self,
        scanning: &RxMemoryListPublished,
    ) -> Result<BluetoothScanStartPublished, ScanStartError> {
        if !scanning.scanning {
            return Err(ScanStartError::NotScanningList);
        }
        if scanning.epoch != self.epoch {
            return Err(ScanStartError::StaleList);
        }
        Ok(self.registers.publish_scan_start())
    }

    /// Publish the device table hardware filters advertising against: its
    /// entry count, then the address of its first entry.
    ///
    /// The table comes from `table`; the caller runs it in the powered
    /// Controller epoch and serializes it with every role that filters
    /// against the table, orders of the Controller's lifecycle.
    #[doc(hidden)]
    pub(crate) fn publish_device_table(
        &mut self,
        table: &DeviceTablePublication,
    ) -> BluetoothDeviceTablePublished {
        self.registers.publish_device_table(table)
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
    pub(crate) fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerHardwareListHeadRetirementObservation {
        self.registers
            .observe_scheduler_hardware_list_head_retirement(run)
    }

    /// Read the current head of one hardware list once, followed by a
    /// device fence.
    pub(crate) fn observe_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        self.registers.observe_scheduler_hardware_list_head(index)
    }

    /// Order prior descriptor writes and publish one scheduler hardware-list
    /// head through the register owner.
    ///
    /// `head` carries the placed-storage memory contract. The engine's
    /// scheduler transaction completes descriptor writes before publication and
    /// retains the submitted graph until completion or removal. The exclusive
    /// register loan serializes this update; the trailing fence orders SRAM
    /// writes before the hardware follows the head.
    #[doc(hidden)]
    pub(crate) fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: Option<crate::layouts::DmaRef<crate::layouts::BleSchedulerItem>>,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        self.registers
            .publish_scheduler_hardware_list_head(index, head)
    }

    /// Clear START in the insertion command selected by the Controller.
    ///
    /// The scheduler transaction selects the command from its retained insertion
    /// state. The exclusive register loan serializes the command update.
    #[doc(hidden)]
    pub(crate) fn clear_scheduler_insertion_command_start(
        &mut self,
        command: BluetoothSchedulerInsertionCommand,
    ) -> BluetoothSchedulerInsertionCommandStartCleared {
        self.registers
            .clear_scheduler_insertion_command_start(command)
    }

    /// Admit one execution lock of the merge-selected item. Nothing is
    /// written until its first step.
    ///
    /// The request carries a typed reference to the merge-selected item in
    /// retained pool storage. The engine's scheduler transaction retains this
    /// execution-lock state through its observation and command-zero START clear.
    #[doc(hidden)]
    pub(crate) fn admit_scheduler_execution_lock(
        &mut self,
        request: BluetoothSchedulerExecutionLockRequest,
    ) -> BluetoothSchedulerExecutionLock {
        BluetoothSchedulerExecutionLock::new(request)
    }

    /// Advance one finite execution-lock step under interrupt serialization:
    /// the engine-idle preamble, publication of command zero and one
    /// observation of it. Pending retains the request; the caller owns its
    /// deadline. Each BUSY sample settles within `settle`.
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) async fn step_scheduler_execution_lock<
        D: engine::scheduler::interrupt::SchedulerDoorbell,
    >(
        &mut self,
        interrupts: &mut engine::scheduler::interrupt::SchedulerTaskHalf,
        lock: &mut BluetoothSchedulerExecutionLock,
        bell: &D,
    ) -> Result<
        BluetoothSchedulerExecutionLockDisposition,
        engine::scheduler::interrupt::SchedulerInterruptError,
    > {
        scheduler_execution_lock::step_hardware(self.registers, interrupts, lock, bell).await
    }

    /// Clear command-zero START of a finished execution lock.
    pub(crate) fn clear_scheduler_execution_lock_start(
        &mut self,
        mut lock: BluetoothSchedulerExecutionLock,
    ) {
        scheduler_execution_lock::clear_start(self.registers, &mut lock);
    }

    /// Admit one execution modify of `index`, in list-deletion mode when
    /// `list_deletion`. Nothing is written until its first step.
    ///
    /// The engine's reconciliation or deletion state selects `index` and retains
    /// this execution-modify state through command-one START clear.
    #[doc(hidden)]
    pub(crate) fn admit_scheduler_execution_modify(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        list_deletion: bool,
    ) -> BluetoothSchedulerExecutionModify {
        BluetoothSchedulerExecutionModify::new(index, list_deletion)
    }

    /// Advance one finite execution-modify step under interrupt
    /// serialization: the engine-idle preamble, publication, the progress
    /// and completion waits, the settle wait and a repeated request after a
    /// conflict. Pending retains the request; the caller owns its deadline.
    /// Each diagnostic sample settles within `settle`.
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) async fn step_scheduler_execution_modify<
        D: engine::scheduler::interrupt::SchedulerDoorbell,
    >(
        &mut self,
        interrupts: &mut engine::scheduler::interrupt::SchedulerTaskHalf,
        modify: &mut BluetoothSchedulerExecutionModify,
        bell: &D,
    ) -> Result<
        BluetoothSchedulerExecutionModifyDisposition,
        engine::scheduler::interrupt::SchedulerInterruptError,
    > {
        scheduler_execution_modify::step_hardware(self.registers, interrupts, modify, bell).await
    }

    /// Clear command-one START of a finished execution modify.
    pub(crate) fn clear_scheduler_execution_modify_start(
        &mut self,
        mut modify: BluetoothSchedulerExecutionModify,
    ) {
        if modify.take_published().is_some() {
            let _cleared = self
                .registers
                .clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::One);
        }
    }

    /// Publish one scheduler skip request through the register owner.
    ///
    /// The request carries a typed reference to the listed item in retained pool
    /// storage. The scheduler transaction retains the returned publication until
    /// observation and clear; the exclusive register loan serializes the writes.
    #[doc(hidden)]
    pub(crate) fn publish_scheduler_skip(
        &mut self,
        request: BluetoothSchedulerSkipRequest,
    ) -> BluetoothSchedulerSkipPublished {
        self.registers.publish_scheduler_skip(request)
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
    /// The scheduler transaction admits this step after lock/modify becomes
    /// idle and retains its affine index token through acknowledgement and
    /// request. The exclusive register loan serializes the operational word.
    #[doc(hidden)]
    pub(crate) fn index_scheduler_cancellation(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerCancellationIndexed {
        self.registers.index_scheduler_cancellation(index)
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

    /// Publish the synchronous BTMAC scheduler event through the register
    /// owner after the matching head and dynamic interrupts are prepared.
    #[doc(hidden)]
    pub(crate) fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        self.registers.publish_scheduler_run_event(head, interrupts)
    }

    /// Publish the final scheduler hardware RUN command through the
    /// register owner. The consumed event retains the complete run prologue.
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
    pub(crate) fn finish_scheduler_software_list_removal(
        &mut self,
        idle: BluetoothSchedulerSoftwareListRemovalIdle,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> BluetoothSchedulerSoftwareListRemovalJoin {
        self.registers
            .finish_scheduler_software_list_removal(idle, head)
    }

    /// Advance the common stop preamble/request/idle sequence under interrupt
    /// serialization. Pending retains the request; the caller owns its deadline.
    /// Each BUSY sample settles within `settle`.
    #[cfg(feature = "bluetooth-engine")]
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn poll_scheduler_stop<D: engine::scheduler::interrupt::SchedulerDoorbell>(
        &mut self,
        interrupts: &mut engine::scheduler::interrupt::SchedulerTaskHalf,
        stop: &mut BluetoothSchedulerStop,
        bell: &D,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<
        Result<
            scheduler_stop::Progress<BluetoothSchedulerStopped>,
            engine::scheduler::interrupt::SchedulerInterruptError,
        >,
    > {
        scheduler_stop::poll_hardware(self.registers, interrupts, stop, bell, cx)
    }

    /// Retire the exact stopped RUN head without clearing a foreign item.
    pub(crate) fn retire_stopped_scheduler_head(
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
    /// The request carries a typed reference to the initialized item in retained
    /// pool storage. The engine's scheduler state retains its publication until
    /// ownership returns; the exclusive register loan serializes these writes.
    #[doc(hidden)]
    pub(crate) fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        self.registers.publish_scheduler_lock_modify(request)
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

/// Sealed conversion from the exclusive task owner to one finite
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
    /// Borrow the controller registers without exposing their register owner.
    fn borrow_bluetooth_controller(&mut self) -> ControllerHal<'_> {
        let owner = sealed::ControllerHalBorrow::bluetooth_task_owner_mut(self);
        owner.reunitable = false;
        ControllerHal {
            registers: &mut owner.registers,
            time_latch: &mut owner.time_latch,
            epoch: owner.epoch,
        }
    }
}

impl ControllerHalBorrow for TaskOwner {}

#[cfg(test)]
mod tests;

/// Controller-memory ownership and scheduler protocol.
#[cfg(feature = "bluetooth-engine")]
pub mod engine;

#[cfg(test)]
fn test_ready<F: core::future::Future>(future: F) -> F::Output {
    match core::pin::pin!(future).poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    )) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("the policy model completes every request immediately"),
    }
}

mod interrupt_policy;
mod observation;
pub use interrupt_policy::{BluetoothInterruptPolicyFault, BluetoothInterruptPolicyObservation};
pub use observation::{BluetoothInterruptObservation, BluetoothTaskInterruptObservation};
