//! Shutdown of a stopped Controller epoch.
//!
//! Its caller first stops its Controller runtime, drains the source-127 task
//! and removes the CPU routes (no composition in this repository does); it
//! then recovers the interrupt output
//! and the ready timer owner from stable ISR storage.
//! [`retire`](crate::shutdown::retire) releases the Controller output and
//! leaves the shared PHY domain and BTBB, as the vendor Controller disable runs
//! `esp_phy_disable(PHY_MODEM_BT)` before its deinit; the shared radio's owner
//! then closes RF if Bluetooth was the last client.
//! [`ControllerRetired::shut_down`](crate::shutdown::ControllerRetired::shut_down) finally resets the Controller domains and
//! returns the clocked client with the BLE PHY allocations, which no hardware
//! references after the reset.

use oer_esp32s31_bluetooth_memory::{BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothControllerOutputReleaseError, BluetoothControllerReset, BluetoothShutdownError,
        BluetoothShutdownFailure, ClockedOwner, InterruptOutputAfterRoutesOwner,
        InterruptOutputReleasedOwner, ModemLpTimerInterruptReadyOwner,
    },
    domain::RadioDomain,
};
use oer_esp32s31_phy::{bluetooth_client::BluetoothPhyClientError, concurrent::ConcurrentPhy};
use oer_interrupt_table::signal::ForeignContext;

use crate::{
    ble_phy::{BlePhyRetainedOwners, DirectionFindingWorkspaceHardwareOwned},
    resources::TaskResources,
    runtime_resources::{
        ControllerEventCells, ControllerPoweredTaskRuntime, ControllerTaskRetirementError,
    },
};

/// First failed step of the Controller shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerShutdownError {
    /// A scheduler or controller-time worker still holds work.
    Task(ControllerTaskRetirementError),
    /// The idle-Controller check before releasing the output failed.
    Output(BluetoothControllerOutputReleaseError),
    /// The shared PHY domain rejected the release.
    Phy(BluetoothPhyClientError),
    /// The Controller reset was rejected or did not read back.
    Reset(BluetoothShutdownError),
    /// The shutdown ran outside the event cells' executor, which alone
    /// discards their notifications; the Controller was not reset.
    ForeignExecutor,
}

/// Failed shutdown retaining every owner it received.
///
/// A rejection leaves hardware as it was, but the running epoch has already
/// lost its routes and runtime, so no transition resumes it; the owners stay
/// retained until the chip resets.
#[must_use = "a failed shutdown retains the Controller until the chip resets"]
pub struct ControllerShutdownFailure {
    error: ControllerShutdownError,
    _retained: Retained,
}

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the retained owners are never read or moved again"
)]
enum Retained {
    Task {
        task: ControllerPoweredTaskRuntime<'static>,
        retained: BlePhyRetainedOwners,
        output: InterruptOutputAfterRoutesOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    },
    Output {
        task: TaskResources,
        retained: BlePhyRetainedOwners,
        output: InterruptOutputAfterRoutesOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    },
    Phy {
        task: TaskResources,
        output: InterruptOutputReleasedOwner,
        timer: ModemLpTimerInterruptReadyOwner,
        membership: oer_esp32s31_phy::bluetooth_client::BluetoothPhyLeaveFailure,
        storage: BlePhyEngineCpuOwned,
        direction_finding: DirectionFindingWorkspaceHardwareOwned,
    },
    Reset {
        failure: BluetoothShutdownFailure,
        storage: BlePhyEngineCpuOwned,
        direction_finding: DirectionFindingWorkspaceHardwareOwned,
    },
    Retired(ControllerRetired),
}

impl ControllerShutdownFailure {
    pub const fn error(&self) -> ControllerShutdownError {
        self.error
    }
}

impl core::fmt::Debug for ControllerShutdownFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ControllerShutdownFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A retired Controller outside the shared PHY domain, awaiting its reset.
#[must_use = "the retired Controller must be shut down"]
pub struct ControllerRetired {
    task: TaskResources,
    output: InterruptOutputReleasedOwner,
    timer: ModemLpTimerInterruptReadyOwner,
    storage: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceHardwareOwned,
}

/// The clocked client and the BLE PHY allocations of a shut-down epoch.
#[must_use = "the clocked client must release its clocks or start another epoch"]
pub struct ControllerShutDown {
    /// The Bluetooth client with its clocks still enabled.
    pub clocked: ClockedOwner,
    /// The BLE PHY environment and resolving-list storage, back at its
    /// allocation-time image.
    pub ble_phy: BlePhyEngineCpuOwned,
    /// The direction-finding workspace, back at its allocation-time image.
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
    /// Proof of the reset, which returns the role memory of the finished
    /// epoch to its initial image as well.
    pub reset: BluetoothControllerReset,
}

/// Retire a stopped epoch: take the task owner from its endpoint, release
/// the Controller output and leave the shared PHY domain and BTBB. Returns
/// whether Bluetooth was the last PHY client.
///
/// # Errors
///
/// A worker still holds work, the Controller is not idle, or the domain
/// rejected the release.
#[allow(
    clippy::result_large_err,
    reason = "the failure retains every owner of the epoch"
)]
pub fn retire(
    radio: &mut RadioDomain<ConcurrentPhy>,
    task: ControllerPoweredTaskRuntime<'static>,
    retained: BlePhyRetainedOwners,
    output: InterruptOutputAfterRoutesOwner,
    timer: ModemLpTimerInterruptReadyOwner,
) -> Result<(ControllerRetired, bool), ControllerShutdownFailure> {
    let mut task = match task.retire() {
        Ok(task) => task,
        Err((error, task)) => {
            return Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Task(error),
                _retained: Retained::Task {
                    task,
                    retained,
                    output,
                    timer,
                },
            });
        }
    };
    let output = match task.release_controller_output(output) {
        Ok(output) => output,
        Err((error, output)) => {
            return Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Output(error),
                _retained: Retained::Output {
                    task,
                    retained,
                    output,
                    timer,
                },
            });
        }
    };
    let BlePhyRetainedOwners {
        membership,
        storage,
        direction_finding,
    } = retained;
    match crate::phy::leave_phy(radio, &task, membership) {
        Ok(last) => Ok((
            ControllerRetired {
                task,
                output,
                timer,
                storage,
                direction_finding,
            },
            last,
        )),
        Err(failure) => Err(ControllerShutdownFailure {
            error: ControllerShutdownError::Phy(failure.error()),
            _retained: Retained::Phy {
                task,
                output,
                timer,
                membership: failure,
                storage,
                direction_finding,
            },
        }),
    }
}

impl ControllerRetired {
    /// Reset the Controller domains, reunite the drained timer and return
    /// the clocked client with the BLE PHY allocations.
    ///
    /// `cells` are the event cells of the retired epoch; their stale
    /// notifications are discarded so the next epoch starts quiet. Only
    /// their executor's context discards them, so the shutdown runs there.
    ///
    /// # Errors
    ///
    /// The shutdown ran outside the cells' executor, or the reset was
    /// rejected or did not read back.
    #[allow(
        clippy::result_large_err,
        reason = "the failure retains every owner of the epoch"
    )]
    pub fn shut_down<T>(
        self,
        radio: &mut RadioDomain<T>,
        cells: &ControllerEventCells,
    ) -> Result<ControllerShutDown, ControllerShutdownFailure> {
        // Before the reset: a foreign context could not discard the
        // notifications after it.
        if let Err(ForeignContext) = cells.is_quiet() {
            return Err(ControllerShutdownFailure {
                error: ControllerShutdownError::ForeignExecutor,
                _retained: Retained::Retired(self),
            });
        }
        let Self {
            task,
            output,
            timer,
            storage,
            direction_finding,
        } = self;
        match task.shut_down(radio, output, timer) {
            Ok((clocked, reset)) => {
                // The check above ran in this same executor context.
                let _cleared = cells.clear_notifications_after_reset();
                let mut ble_phy = storage;
                ble_phy.reset_after_controller_reset(&reset);
                let mut direction_finding = direction_finding.storage;
                direction_finding.reset_after_controller_reset(&reset);
                Ok(ControllerShutDown {
                    clocked,
                    ble_phy,
                    direction_finding,
                    reset,
                })
            }
            Err(failure) => Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Reset(failure.error()),
                _retained: Retained::Reset {
                    failure,
                    storage,
                    direction_finding,
                },
            }),
        }
    }
}
