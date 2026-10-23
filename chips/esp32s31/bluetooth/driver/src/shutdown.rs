//! Shutdown of a stopped Controller epoch.
//!
//! Its caller first stops its Controller runtime, drains the source-127 task
//! and removes the CPU routes (no composition in this repository does); it
//! then recovers the interrupt output
//! and the ready timer owner from stable ISR storage.
//! [`retire`](crate::shutdown::retire) releases the Controller output and
//! returns a drained Controller to the shared actor. The actor leaves the
//! shared PHY domain and BTBB before reset, releases clocks/coexistence/power,
//! and closes RF only after the last client leaves.
//! [`ControllerRetired::shut_down`](crate::shutdown::ControllerRetired::shut_down) finally resets the Controller domains and
//! returns the clocked client with the complete controller arena, which no hardware
//! references after the reset.

use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;

use oer_esp32s31_hal::power::PlatformClockBackend;

use oer_esp32s31_hal::bluetooth::engine::{
    EngineShutDown, EngineShutdownError, EngineShutdownFailure,
};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothControllerOutputReleaseError, InterruptOutputAfterRoutesOwner,
        InterruptOutputReleasedOwner, ModemLpTimerInterruptReadyOwner,
    },
    domain::RadioDomain,
};
use oer_interrupt_table::signal::{ForeignContext, SignalExecutor};

use crate::{
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
    /// The Controller reset was rejected or did not read back.
    Reset(EngineShutdownError),
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
pub struct ControllerShutdownFailure<X: SignalExecutor, M: ControllerMemory> {
    error: ControllerShutdownError,
    _retained: Retained<X, M>,
}

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the retained owners are never read or moved again"
)]
enum Retained<X: SignalExecutor, M: ControllerMemory> {
    Task {
        task: ControllerPoweredTaskRuntime<'static, X, M>,
        output: InterruptOutputAfterRoutesOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    },
    Output {
        task: TaskResources<M>,
        output: InterruptOutputAfterRoutesOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    },
    Reset {
        failure: EngineShutdownFailure<M>,
    },
    Retired(ControllerRetired<M>),
}

impl<X: SignalExecutor, M: ControllerMemory> ControllerShutdownFailure<X, M> {
    pub const fn error(&self) -> ControllerShutdownError {
        self.error
    }
}

impl<X: SignalExecutor, M: ControllerMemory> core::fmt::Debug for ControllerShutdownFailure<X, M> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ControllerShutdownFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A retired Controller awaiting the actor's PHY/BTBB leave and reset.
#[must_use = "the retired Controller must be shut down"]
pub struct ControllerRetired<M: ControllerMemory> {
    task: TaskResources<M>,
    output: InterruptOutputReleasedOwner,
    timer: ModemLpTimerInterruptReadyOwner,
}

/// Clocked client and the entire CPU-owned arena after verified reset.
pub type ControllerShutDown<M> = EngineShutDown<M>;

/// Retire a stopped epoch: take the task owner from its endpoint, release
/// the Controller output. The shared actor performs PHY/BTBB leave before
/// the reset. BUSY samples of the output release settle on `clock`.
///
/// # Errors
///
/// A worker still holds work or the Controller is not idle.
#[allow(
    clippy::result_large_err,
    reason = "the failure retains every owner of the epoch"
)]
pub fn retire<X: SignalExecutor, M: ControllerMemory>(
    task: ControllerPoweredTaskRuntime<'static, X, M>,
    output: InterruptOutputAfterRoutesOwner,
    timer: ModemLpTimerInterruptReadyOwner,
    clock: &impl oer_time::Clock,
) -> Result<ControllerRetired<M>, ControllerShutdownFailure<X, M>> {
    let mut task = match task.retire() {
        Ok(task) => task,
        Err((error, task)) => {
            return Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Task(error),
                _retained: Retained::Task {
                    task,
                    output,
                    timer,
                },
            });
        }
    };
    let output = match task.release_controller_output(output, clock) {
        Ok(output) => output,
        Err((error, output)) => {
            return Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Output(error),
                _retained: Retained::Output {
                    task,
                    output,
                    timer,
                },
            });
        }
    };
    Ok(ControllerRetired {
        task,
        output,
        timer,
    })
}

impl<M: ControllerMemory> ControllerRetired<M> {
    /// Identity checked before a retirement can satisfy an actor leave.
    pub fn installation(&self) -> u32 {
        self.task.installation()
    }

    /// Borrow authority for the actor's PHY/BTBB leave before reset.
    pub fn task_owner(&self) -> &oer_esp32s31_hal::bluetooth::TaskOwner {
        self.task.hal()
    }

    /// Reset the Controller domains, reunite the drained timer and return
    /// the clocked client with the complete controller arena.
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
    pub fn shut_down<X: SignalExecutor, B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        cells: &ControllerEventCells<X>,
    ) -> Result<ControllerShutDown<M>, ControllerShutdownFailure<X, M>> {
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
        } = self;
        match task.shut_down(radio, output, timer) {
            Ok(returned) => {
                let _cleared = cells.clear_notifications_after_reset();
                Ok(returned)
            }
            Err(failure) => Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Reset(failure.error()),
                _retained: Retained::Reset { failure },
            }),
        }
    }
}
