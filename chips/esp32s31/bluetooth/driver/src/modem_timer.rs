//! Source-127 timer ownership, readiness and finite task transitions.
//!
//! Boot supplies the timer runtime once. This module retains the queue and HAL
//! phase across task waits and exchanges owners with the timer's gate in the
//! Bluetooth interrupt handler ([`crate::interrupt::ModemLpTimerPort`]); it
//! does not own Controller scheduling or HCI command dispatch.

use crate::{
    modem_lp_timer_queue::{
        ModemLpTimerEventCell, ModemLpTimerEventPublication, ModemLpTimerExpiration,
        ModemLpTimerExpirationState, ModemLpTimerSoftwareState, ModemLpTimerSoftwareStateStep,
    },
    runtime_resources::ControllerModemTimerRuntime,
};
use oer_esp32s31_hal::bluetooth::{
    ModemLpTimerInterruptReadyOwner, ModemLpTimerSoftwarePendingOwner,
};
use oer_interrupt_table::signal::{ForeignContext, SignalExecutor};

/// Stable-storage boundary for source-127 task ownership.
///
/// The timer's gate implements it ([`crate::interrupt::ModemLpTimerPort`]).
/// Taking an owner leaves the gate empty, so repeated interrupt entry cannot
/// touch MMIO while task work owns the timer. Only a fully rearmed owner may
/// be restored.
pub trait ModemLpTimerSoftwareOwnerStorage {
    /// Exact reason task context could not acquire pending work.
    type TakeError;
    /// Exact reason the fully rearmed owner could not return to ISR storage.
    type RestoreError;

    /// Move software-pending ownership out of stable ISR storage.
    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError>;

    /// Restore only a fully rearmed owner, retaining it on rejection.
    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)>;
}

/// Why the timer's gate refused a software exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemLpTimerPortError {
    /// The handler handed on no pending owner.
    NoPendingOwner,
    /// The handler has not taken the previous rearmed owner.
    Occupied,
}

impl<R: crate::interrupt::RadioDoorbell> ModemLpTimerSoftwareOwnerStorage
    for crate::interrupt::ModemLpTimerPort<crate::interrupt::ChipGate, R>
{
    type TakeError = ModemLpTimerPortError;
    type RestoreError = ModemLpTimerPortError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, ModemLpTimerPortError> {
        self.take_pending()
            .ok_or(ModemLpTimerPortError::NoPendingOwner)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (ModemLpTimerPortError, ModemLpTimerInterruptReadyOwner)> {
        self.restore_ready(owner)
            .map_err(|owner| (ModemLpTimerPortError::Occupied, owner))
    }
}

/// Stable state retained inside the disjoint source-127 task endpoint.
enum ControllerModemTimerTaskPhase {
    Idle,
    Work(ModemLpTimerSoftwareState),
    Expiration(ModemLpTimerExpirationState),
    Rearm(ModemLpTimerInterruptReadyOwner),
}

/// Borrowed readiness class for the source-127 task endpoint.
///
/// The value carries no timer owner and may be held by an executor wait future.
/// The affine queue, epoch and HAL phase remain inside
/// [`ControllerModemTimerTask`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerModemTimerReadinessClass {
    /// No interrupt-owned software work is known to be pending.
    Interrupt,
    /// One finite software transition can run immediately.
    Step,
    /// Expiration publication is waiting for the one-event cell to become empty.
    EventCapacity,
    /// A fully rearmed owner is ready for stable-storage restoration.
    Rearm,
}

/// Owner-free borrowed readiness observation for source 127.
#[must_use = "readiness must be checked before polling again"]
pub struct ControllerModemTimerReadiness<'task, X: SignalExecutor> {
    class: ControllerModemTimerReadinessClass,
    worker_wake: &'task crate::modem_lp_timer_queue::ModemLpTimerWorkerWakeCell<X>,
    events: &'task ModemLpTimerEventCell,
}

impl<X: SignalExecutor> ControllerModemTimerReadiness<'_, X> {
    /// Exact state an executor wait is observing.
    pub const fn class(&self) -> ControllerModemTimerReadinessClass {
        self.class
    }

    /// Recheck readiness after registering the executor's own waker; for
    /// [`ControllerModemTimerReadinessClass::Interrupt`] the worker wake's
    /// [`poll_pending`](crate::modem_lp_timer_queue::ModemLpTimerWorkerWakeCell::poll_pending)
    /// registers it.
    ///
    /// This operation is borrowed and value-only. It neither acquires stable
    /// ownership nor advances queue or register state.
    ///
    /// # Errors
    ///
    /// The caller runs outside the worker wake's executor.
    pub fn is_ready(&self) -> Result<bool, ForeignContext> {
        Ok(match self.class {
            ControllerModemTimerReadinessClass::Interrupt => self.worker_wake.is_pending()?,
            ControllerModemTimerReadinessClass::Step
            | ControllerModemTimerReadinessClass::Rearm => true,
            ControllerModemTimerReadinessClass::EventCapacity => !self.events.is_pending(),
        })
    }
}

/// Result of acquiring one durable source-127 software-pending owner.
#[must_use = "a begin result must be handled without losing stable readiness"]
pub enum ControllerModemTimerBegin<E> {
    /// No durable interrupt publication currently requests task work.
    NotReady,
    /// The owner entered the endpoint and one finite software step is ready.
    Started,
    /// Stable storage rejected acquisition; the endpoint remains idle.
    StorageRejected(E),
    /// A software or rearm phase is already retained by this endpoint.
    AlreadyActive,
    /// The task runs outside its worker wake's executor; nothing changed.
    ForeignExecutor,
}

/// Result of one finite source-127 task transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the next source-127 readiness class must be observed"]
pub enum ControllerModemTimerStep {
    /// No software owner has been acquired.
    Idle,
    /// One expiration now owns the durable publication edge.
    ExpirationPending(ModemLpTimerExpiration),
    /// The expiration was published and later work remains retained.
    Published(ModemLpTimerEventPublication),
    /// The expiration cell is occupied and the unchanged publication remains retained.
    Backpressured(ModemLpTimerExpiration),
    /// Immediate compare disposition requires a later fresh finite recheck.
    Recheck,
    /// Queue processing produced a fully rearmed owner retained inside the endpoint.
    RearmPending,
}

/// Result of restoring the fully rearmed owner to stable ISR storage.
#[must_use = "a rejected rearm remains retained by the timer task"]
pub enum ControllerModemTimerRearm<E> {
    /// The ready owner is back in stable interrupt storage.
    Rearmed,
    /// Stable storage rejected restoration; the exact ready owner remains private.
    StorageRejected(E),
    /// No fully rearmed owner is currently retained.
    NotReady,
    /// The task runs outside its worker wake's executor; the owner stays
    /// retained.
    ForeignExecutor,
}

/// Disjoint task-context source-127 owner for one final Controller runtime.
///
/// This endpoint exclusively owns the mutable timer queue and positional epoch.
/// Stable storage is shared only as the platform exchange boundary with the
/// interrupt service. All affine HAL states remain private across borrowed
/// readiness and finite `begin`, `step`, and `rearm` calls, so an executor
/// future never needs to own them.
#[must_use = "the modem timer task retains source-127 queue and hardware ownership"]
pub struct ControllerModemTimerTask<'runtime, S, const CAPACITY: usize, X: SignalExecutor> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY, X>,
    phase: ControllerModemTimerTaskPhase,
}

impl<'runtime, S, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerTask<'runtime, S, CAPACITY, X>
where
    S: ModemLpTimerSoftwareOwnerStorage,
{
    pub fn new(
        storage: &'runtime S,
        runtime: ControllerModemTimerRuntime<'runtime, CAPACITY, X>,
    ) -> Self {
        Self {
            storage,
            runtime,
            phase: ControllerModemTimerTaskPhase::Idle,
        }
    }

    /// Borrow the current owner-free readiness predicate.
    pub fn readiness(&self) -> ControllerModemTimerReadiness<'_, X> {
        let class = match &self.phase {
            ControllerModemTimerTaskPhase::Idle => ControllerModemTimerReadinessClass::Interrupt,
            ControllerModemTimerTaskPhase::Work(_) => ControllerModemTimerReadinessClass::Step,
            ControllerModemTimerTaskPhase::Expiration(_) => {
                ControllerModemTimerReadinessClass::EventCapacity
            }
            ControllerModemTimerTaskPhase::Rearm(_) => ControllerModemTimerReadinessClass::Rearm,
        };
        ControllerModemTimerReadiness {
            class,
            worker_wake: self.runtime.worker_wake(),
            events: self.runtime.events(),
        }
    }

    /// Acquire exactly one software-pending owner after borrowed readiness.
    pub fn begin(&mut self) -> ControllerModemTimerBegin<S::TakeError> {
        if !matches!(self.phase, ControllerModemTimerTaskPhase::Idle) {
            return ControllerModemTimerBegin::AlreadyActive;
        }
        match self.runtime.worker_wake().is_pending() {
            Ok(true) => {}
            Ok(false) => return ControllerModemTimerBegin::NotReady,
            Err(ForeignContext) => return ControllerModemTimerBegin::ForeignExecutor,
        }
        match self.storage.take_modem_lp_timer_software_pending() {
            Ok(owner) => {
                // The check above ran in this same executor context.
                let _taken = self.runtime.worker_wake.take();
                self.phase = ControllerModemTimerTaskPhase::Work(ModemLpTimerSoftwareState::begin(
                    owner,
                    &mut self.runtime.epoch,
                ));
                ControllerModemTimerBegin::Started
            }
            Err(error) => ControllerModemTimerBegin::StorageRejected(error),
        }
    }

    /// Advance exactly one queue, publication or compare transition.
    pub fn step(&mut self) -> ControllerModemTimerStep {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        match phase {
            ControllerModemTimerTaskPhase::Idle => ControllerModemTimerStep::Idle,
            ControllerModemTimerTaskPhase::Work(work) => {
                match work.step(&mut self.runtime.queue, &mut self.runtime.epoch) {
                    ModemLpTimerSoftwareStateStep::Expiration(pending) => {
                        let event = pending.event();
                        self.phase = ControllerModemTimerTaskPhase::Expiration(pending);
                        ControllerModemTimerStep::ExpirationPending(event)
                    }
                    ModemLpTimerSoftwareStateStep::Recheck(work) => {
                        self.phase = ControllerModemTimerTaskPhase::Work(work);
                        ControllerModemTimerStep::Recheck
                    }
                    ModemLpTimerSoftwareStateStep::Rearmed(owner) => {
                        self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                        ControllerModemTimerStep::RearmPending
                    }
                }
            }
            ControllerModemTimerTaskPhase::Expiration(pending) => {
                let event = pending.event();
                match pending.publish(self.runtime.events()) {
                    Ok((work, publication)) => {
                        self.phase = ControllerModemTimerTaskPhase::Work(work);
                        ControllerModemTimerStep::Published(publication)
                    }
                    Err(pending) => {
                        self.phase = ControllerModemTimerTaskPhase::Expiration(pending);
                        ControllerModemTimerStep::Backpressured(event)
                    }
                }
            }
            ControllerModemTimerTaskPhase::Rearm(owner) => {
                self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                ControllerModemTimerStep::RearmPending
            }
        }
    }

    /// Restore one fully rearmed owner to stable source-127 interrupt storage.
    pub fn rearm(&mut self) -> ControllerModemTimerRearm<S::RestoreError> {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        let ControllerModemTimerTaskPhase::Rearm(owner) = phase else {
            self.phase = phase;
            return ControllerModemTimerRearm::NotReady;
        };
        if let Err(ForeignContext) = self.runtime.worker_wake.take() {
            self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
            return ControllerModemTimerRearm::ForeignExecutor;
        }
        match self.storage.restore_modem_lp_timer_ready(owner) {
            Ok(()) => ControllerModemTimerRearm::Rearmed,
            Err((error, owner)) => {
                self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                ControllerModemTimerRearm::StorageRejected(error)
            }
        }
    }

    /// Consume one durably published expiration, if present.
    pub fn take_expiration(&mut self) -> Option<ModemLpTimerExpiration> {
        self.runtime.events().take()
    }
}

/// Drained software timer task; its HAL owner stays with the timer's
/// interrupt handler, which disables the compare and keeps the owner until
/// the Controller reset.
///
/// This state is not a timer reset, BTBB stop, or permission to release PHY.
/// Queue and positional epoch borrows retain their original identity.
#[must_use = "retain the timer runtime with its Controller epoch"]
pub struct ControllerModemTimerRetired<'runtime, S, const CAPACITY: usize, X: SignalExecutor> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY, X>,
}

impl<'runtime, S, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerRetired<'runtime, S, CAPACITY, X>
{
    pub fn matches_storage(&self, storage: &S) -> bool {
        core::ptr::eq(self.storage, storage)
    }

    pub fn into_shutdown_parts(
        self,
    ) -> (
        ControllerModemTimerRuntime<'runtime, CAPACITY, X>,
        &'runtime S,
    ) {
        (self.runtime, self.storage)
    }
}

impl<'runtime, S: ModemLpTimerSoftwareOwnerStorage, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerTask<'runtime, S, CAPACITY, X>
{
    /// Observe whether task work has drained before stopping the timer.
    ///
    /// This takes no hardware owner. An ISR can publish work after this check;
    /// `try_retire` rechecks. Outside the worker wake's executor the task is
    /// never ready: only that context observes the handoff.
    pub fn retirement_ready(&self) -> bool {
        self.drained().is_ok()
    }

    fn drained(
        &self,
    ) -> Result<
        (),
        crate::modem_timer_retirement::ControllerModemTimerRetirementError<
            core::convert::Infallible,
        >,
    > {
        crate::modem_timer_retirement::retire_when_drained(
            matches!(self.phase, ControllerModemTimerTaskPhase::Idle),
            self.runtime.queue_is_empty(),
            self.runtime.worker_wake().is_pending(),
            self.runtime.events().is_pending(),
            || Ok::<_, core::convert::Infallible>(()),
        )
    }

    /// Retire the software task only after all its work drained; the
    /// hardware owner stays with the timer's handler. Rejection retains the
    /// entire task, including active HAL phases and events.
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains all affine task phases"
    )]
    pub fn try_retire(
        self,
    ) -> Result<
        ControllerModemTimerRetired<'runtime, S, CAPACITY, X>,
        (
            crate::modem_timer_retirement::ControllerModemTimerRetirementError<
                core::convert::Infallible,
            >,
            Self,
        ),
    > {
        match self.drained() {
            Ok(()) => Ok(ControllerModemTimerRetired {
                storage: self.storage,
                runtime: self.runtime,
            }),
            Err(error) => Err((error, self)),
        }
    }
}

impl<'runtime, S: ModemLpTimerSoftwareOwnerStorage, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerRetired<'runtime, S, CAPACITY, X>
{
    /// Resume the same software epoch; its owner never left the handler.
    pub fn resume(self) -> ControllerModemTimerTask<'runtime, S, CAPACITY, X> {
        ControllerModemTimerTask::new(self.storage, self.runtime)
    }
}
