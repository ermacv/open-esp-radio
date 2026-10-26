//! Shutdown of a stopped Controller epoch.
//!
//! Its caller first stops its Controller runtime and drains the source-127
//! task. [`retire`] then releases the Controller output with the bank's
//! interrupt handler, closes the bank's gate (the released bank and the
//! handler's scheduler endpoint leave the handler), retires the scheduler
//! channel and has the timer's handler stop the counter and both measured
//! delivery lanes; the interrupt route stays. The shared actor leaves the shared PHY domain and BTBB before
//! reset, releases clocks/coexistence/power, and closes RF only after the last
//! client leaves. [`ControllerRetired::shut_down`] resets the Controller
//! domains, and [`ControllerResetTimerPending::reunite`] takes the timer from
//! its handler and returns the clocked client with the complete controller
//! arena, which no hardware references after the reset.

use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;

use oer_esp32s31_hal::power::PlatformClockBackend;

use oer_esp32s31_hal::bluetooth::engine::{
    EngineOutputReleaseError, EngineResetTimerPending, EngineShutDown, EngineShutdownError,
    EngineShutdownFailure, scheduler::interrupt::SchedulerDoorbell,
};
use oer_esp32s31_hal::{
    bluetooth::{InterruptOutputReleasedOwner, ModemLpTimerStopped},
    domain::RadioDomain,
};
use oer_interrupt_table::signal::{ForeignContext, SignalExecutor};

use crate::{
    interrupt::{BluetoothInterruptGate, ChipGate, GateError, RadioDoorbell},
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
    /// The release of the controller output refused or its exchange failed.
    Output(EngineOutputReleaseError),
    /// The interrupt handler's gate refused or its exchange failed.
    Gate(GateError),
    /// The Controller reset was rejected or did not read back.
    Reset(EngineShutdownError),
    /// The shutdown ran outside the event cells' executor, which alone
    /// discards their notifications; the Controller was not reset.
    ForeignExecutor,
}

/// Failed shutdown retaining every owner it received.
///
/// A rejection leaves hardware as it was, but the running epoch has already
/// lost its runtime, so no transition resumes it; the owners stay retained
/// (an owner its handler still holds stays there) until the chip resets.
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
    },
    Output {
        task: TaskResources<M>,
        output: InterruptOutputReleasedOwner,
    },
    Reset {
        failure: EngineShutdownFailure<M>,
    },
    Retired(ControllerRetired<M>),
    ResetTimerPending(ControllerResetTimerPending<M>),
}

impl<X: SignalExecutor, M: ControllerMemory> ControllerShutdownFailure<X, M> {
    /// Observation is present once LP stop succeeded, even if a later step refused.
    pub const fn pending_observation(
        &self,
    ) -> Option<oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation> {
        match &self._retained {
            Retained::Retired(retired) => Some(retired.pending_observation()),
            Retained::Reset { failure } => Some(failure.pending_observation()),
            Retained::ResetTimerPending(pending) => Some(pending.pending_observation()),
            Retained::Task { .. } | Retained::Output { .. } => None,
        }
    }
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

/// A retired Controller awaiting the actor's PHY/BTBB leave and reset: its
/// output is released and its timer's counter and both measured lanes are stopped.
#[must_use = "the retired Controller must be shut down"]
pub struct ControllerRetired<M: ControllerMemory> {
    task: TaskResources<M>,
    output: InterruptOutputReleasedOwner,
    stopped: ModemLpTimerStopped,
}

/// The reset Controller, awaiting the timer its handler gives up.
#[must_use = "the reset Controller returns its arena with its timer"]
pub struct ControllerResetTimerPending<M: ControllerMemory> {
    engine: EngineResetTimerPending<M>,
}

/// Clocked client and the entire CPU-owned arena after verified reset.
pub type ControllerShutDown<M> = EngineShutDown<M>;

/// Retire a stopped epoch: release the Controller output with the bank's
/// handler over the scheduler channel `bell` reaches, close the bank's gate,
/// retire the channel and have the timer's handler stop its counter and both
/// measured delivery lanes. `timer_port` is the retired task's endpoint of
/// this gate, whose late rollover-only snapshot is discarded after stop.
/// BUSY samples of the release settle in the handler.
///
/// # Errors
///
/// A worker still holds work, the Controller is not idle, or the gate
/// refused; an owner its handler still holds stays there.
#[allow(
    clippy::result_large_err,
    reason = "the failure retains every owner of the epoch"
)]
pub async fn retire<X: SignalExecutor, M: ControllerMemory, R: RadioDoorbell>(
    mut task: ControllerPoweredTaskRuntime<'static, X, M>,
    gate: &mut BluetoothInterruptGate<ChipGate, X, R>,
    bell: &impl SchedulerDoorbell,
    timer_port: &crate::interrupt::ModemLpTimerPort<ChipGate, R>,
) -> Result<ControllerRetired<M>, ControllerShutdownFailure<X, M>> {
    let failed = |error, task| {
        Err(ControllerShutdownFailure {
            error,
            _retained: Retained::Task { task },
        })
    };
    if let Err(error) = task.stopped_for_output_release() {
        return failed(ControllerShutdownError::Task(error), task);
    }
    let released = match task.task_owner().release_controller_output(bell).await {
        Ok(released) => released,
        Err(error) => return failed(ControllerShutdownError::Output(error), task),
    };
    let (output, scheduler) = match gate.close(released).await {
        Ok(closed) => closed,
        Err(error) => return failed(ControllerShutdownError::Gate(error), task),
    };
    if let Err((error, _scheduler)) = task
        .task_owner()
        .engine()
        .retire_scheduler_interrupts(scheduler)
    {
        return failed(
            ControllerShutdownError::Output(EngineOutputReleaseError::Channel(error)),
            task,
        );
    }
    let task = match task.retire() {
        Ok(task) => task,
        Err((error, task)) => return failed(ControllerShutdownError::Task(error), task),
    };
    match crate::interrupt::gate::stop_timer_for_retirement(gate, timer_port).await {
        Ok(stopped) => Ok(ControllerRetired {
            task,
            output,
            stopped,
        }),
        Err(error) => Err(ControllerShutdownFailure {
            error: ControllerShutdownError::Gate(error),
            _retained: Retained::Output { task, output },
        }),
    }
}

impl<M: ControllerMemory> ControllerRetired<M> {
    /// Pending sampled before LP quieting; retained across the actor leave.
    pub const fn pending_observation(
        &self,
    ) -> oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation {
        self.stopped.pending_observation()
    }
    /// Identity checked before a retirement can satisfy an actor leave.
    pub fn installation(&self) -> u32 {
        self.task.installation()
    }

    /// Borrow authority for the actor's PHY/BTBB leave before reset.
    pub fn task_owner(&self) -> &oer_esp32s31_hal::bluetooth::TaskOwner {
        self.task.hal()
    }

    /// Reset the Controller domains and await the timer its handler gives
    /// up after the reset.
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
    ) -> Result<ControllerResetTimerPending<M>, ControllerShutdownFailure<X, M>> {
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
            stopped,
        } = self;
        match task.shut_down(radio, output, stopped) {
            Ok(engine) => {
                let _cleared = cells.clear_notifications_after_reset();
                Ok(ControllerResetTimerPending { engine })
            }
            Err(failure) => Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Reset(failure.error()),
                _retained: Retained::Reset { failure },
            }),
        }
    }
}

impl<M: ControllerMemory> ControllerResetTimerPending<M> {
    /// Pending observed before LP quieting, preserved through reset.
    pub const fn pending_observation(
        &self,
    ) -> oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation {
        self.engine.pending_observation()
    }
    /// Take the timer from its handler, which gives it up only after the
    /// reset, and return the clocked client with the complete arena.
    ///
    /// # Errors
    ///
    /// The gate refused or its exchange failed; the timer stays with its
    /// handler.
    #[allow(
        clippy::result_large_err,
        reason = "the failure retains every owner of the epoch"
    )]
    pub async fn reunite<X: SignalExecutor, R: RadioDoorbell>(
        self,
        gate: &mut BluetoothInterruptGate<ChipGate, X, R>,
    ) -> Result<ControllerShutDown<M>, ControllerShutdownFailure<X, M>> {
        match gate.take_timer(&self.engine).await {
            Ok(timer) => Ok(self.engine.reunite(timer)),
            Err(error) => Err(ControllerShutdownFailure {
                error: ControllerShutdownError::Gate(error),
                _retained: Retained::ResetTimerPending(self),
            }),
        }
    }
}
