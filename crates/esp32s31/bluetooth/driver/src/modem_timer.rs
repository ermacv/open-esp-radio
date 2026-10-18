//! Source-127 timer ownership, readiness and finite task transitions.
//!
//! Boot supplies the timer runtime once. This module retains the queue and HAL
//! phase across task waits and exchanges owners with stable ISR storage; it
//! does not own Controller scheduling or HCI command dispatch.

use crate::{
    modem_lp_timer_queue::{
        ModemLpTimerEventCell, ModemLpTimerEventPublication, ModemLpTimerExpiration,
        ModemLpTimerExpirationState, ModemLpTimerSoftwareState, ModemLpTimerSoftwareStateStep,
        ModemLpTimerStableInterruptStep,
    },
    runtime_resources::ControllerModemTimerRuntime,
};
use oer_esp32s31_hal::bluetooth::{
    ModemLpTimerInterruptReadyOwner, ModemLpTimerSoftwarePendingOwner,
};

/// Stable-storage boundary for source-127 task ownership.
///
/// The interrupt platform implements this trait for the same affine lease
/// returned by [`crate::interrupt::InterruptOwnerStorage`]. Taking an owner leaves the
/// stable ISR slot empty, so repeated interrupt entry cannot touch MMIO while
/// task work owns the timer. Only a fully rearmed owner may be restored.
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

/// Stable platform dispatch over the published source-127 interrupt owner.
///
/// Implementations retain either the ready or software-pending affine owner
/// in process-wide storage and perform no executor notification themselves.
pub trait ModemLpTimerInterruptDispatchStorage {
    /// Exact reason the stable owner could not service this entry.
    type Error;

    /// Execute one finite register entry and return its semantic disposition.
    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error>;
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
pub struct ControllerModemTimerReadiness<'task> {
    class: ControllerModemTimerReadinessClass,
    worker_wake: &'task crate::modem_lp_timer_queue::ModemLpTimerWorkerWakeCell,
    events: &'task ModemLpTimerEventCell,
}

impl ControllerModemTimerReadiness<'_> {
    /// Exact state an executor wait is observing.
    pub const fn class(&self) -> ControllerModemTimerReadinessClass {
        self.class
    }

    /// Recheck readiness after registering the executor's own waker.
    ///
    /// This operation is borrowed and value-only. It neither acquires stable
    /// ownership nor advances queue or register state.
    pub fn is_ready(&self) -> bool {
        match self.class {
            ControllerModemTimerReadinessClass::Interrupt => self.worker_wake.is_pending(),
            ControllerModemTimerReadinessClass::Step
            | ControllerModemTimerReadinessClass::Rearm => true,
            ControllerModemTimerReadinessClass::EventCapacity => !self.events.is_pending(),
        }
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
}

/// Disjoint task-context source-127 owner for one final Controller runtime.
///
/// This endpoint exclusively owns the mutable timer queue and positional epoch.
/// Stable storage is shared only as the platform exchange boundary with the
/// interrupt service. All affine HAL states remain private across borrowed
/// readiness and finite `begin`, `step`, and `rearm` calls, so an executor
/// future never needs to own them.
#[must_use = "the modem timer task retains source-127 queue and hardware ownership"]
pub struct ControllerModemTimerTask<'runtime, S, const CAPACITY: usize> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>,
    phase: ControllerModemTimerTaskPhase,
}

impl<'runtime, S, const CAPACITY: usize> ControllerModemTimerTask<'runtime, S, CAPACITY>
where
    S: ModemLpTimerSoftwareOwnerStorage,
{
    pub fn new(
        storage: &'runtime S,
        runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>,
    ) -> Self {
        Self {
            storage,
            runtime,
            phase: ControllerModemTimerTaskPhase::Idle,
        }
    }

    /// Borrow the current owner-free readiness predicate.
    pub fn readiness(&self) -> ControllerModemTimerReadiness<'_> {
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
        if !self.runtime.worker_wake().is_pending() {
            return ControllerModemTimerBegin::NotReady;
        }
        match self.storage.take_modem_lp_timer_software_pending() {
            Ok(owner) => {
                self.runtime.worker_wake.take();
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
        self.runtime.worker_wake.take();
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

/// Platform boundary for extracting the ready timer after complete IRQ disable.
///
/// Implementations serialize extraction with binding and interrupt service,
/// reject any bound route epoch (including quarantine), and return only a ready
/// owner. Rebinding must reject absent stable owners until explicit restoration.
pub trait ModemLpTimerRetirementStorage: ModemLpTimerSoftwareOwnerStorage {
    /// Exact route or stable-owner rejection, without storage mutation.
    type RetireError;

    /// Take the ready owner only after every route and in-flight handler stopped.
    fn take_modem_lp_timer_ready_after_routes_disabled(
        &self,
    ) -> Result<ModemLpTimerInterruptReadyOwner, Self::RetireError>;
}

/// Drained software timer task with its actual HAL owner removed from ISR storage.
///
/// The started hardware counter remains owned here; this state is not a timer
/// reset, BTBB stop, or permission to release PHY. Queue and positional epoch
/// borrows retain their original identity. `resume` restores this exact owner
/// before CPU routes can be bound again.
#[must_use = "retain the timer hardware and its originating runtime together"]
pub struct ControllerModemTimerRetired<'runtime, S, const CAPACITY: usize> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>,
    owner: ModemLpTimerInterruptReadyOwner,
}

impl<'runtime, S, const CAPACITY: usize> ControllerModemTimerRetired<'runtime, S, CAPACITY> {
    pub fn matches_storage(&self, storage: &S) -> bool {
        core::ptr::eq(self.storage, storage)
    }

    pub fn from_maintenance_parts(
        owner: ModemLpTimerInterruptReadyOwner,
        runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>,
        storage: &'runtime S,
    ) -> Self {
        Self {
            storage,
            runtime,
            owner,
        }
    }

    pub fn into_shutdown_parts(
        self,
    ) -> (
        ModemLpTimerInterruptReadyOwner,
        ControllerModemTimerRuntime<'runtime, CAPACITY>,
        &'runtime S,
    ) {
        (self.owner, self.runtime, self.storage)
    }
}

impl<'runtime, S: ModemLpTimerRetirementStorage, const CAPACITY: usize>
    ControllerModemTimerTask<'runtime, S, CAPACITY>
{
    /// Observe whether task work has drained before attempting IRQ shutdown.
    ///
    /// This takes no hardware owner. An ISR can publish work after this check;
    /// `try_retire` must recheck after all routes have been disabled.
    pub fn retirement_ready(&self) -> bool {
        crate::modem_timer_retirement::retire_when_drained(
            matches!(self.phase, ControllerModemTimerTaskPhase::Idle),
            self.runtime.queue_is_empty(),
            self.runtime.worker_wake().is_pending(),
            self.runtime.events().is_pending(),
            || Ok::<_, core::convert::Infallible>(()),
        )
        .is_ok()
    }

    /// Return hardware and software ownership only after all task work drains.
    /// Rejection retains the entire task, including active HAL phases and events.
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains all affine task phases"
    )]
    pub fn try_retire(
        self,
    ) -> Result<
        ControllerModemTimerRetired<'runtime, S, CAPACITY>,
        (
            crate::modem_timer_retirement::ControllerModemTimerRetirementError<S::RetireError>,
            Self,
        ),
    > {
        let owner = crate::modem_timer_retirement::retire_when_drained(
            matches!(self.phase, ControllerModemTimerTaskPhase::Idle),
            self.runtime.queue_is_empty(),
            self.runtime.worker_wake().is_pending(),
            self.runtime.events().is_pending(),
            || {
                self.storage
                    .take_modem_lp_timer_ready_after_routes_disabled()
            },
        );
        match owner {
            Ok(owner) => Ok(ControllerModemTimerRetired {
                storage: self.storage,
                runtime: self.runtime,
                owner,
            }),
            Err(error) => Err((error, self)),
        }
    }
}

impl<'runtime, S: ModemLpTimerSoftwareOwnerStorage, const CAPACITY: usize>
    ControllerModemTimerRetired<'runtime, S, CAPACITY>
{
    /// Restore the same timer owner and software epoch before rebinding IRQ.
    /// Rejection retains the complete retired task for a corrected retry.
    #[allow(
        clippy::result_large_err,
        reason = "restore rejection returns the exact HAL owner"
    )]
    pub fn resume(
        self,
    ) -> Result<ControllerModemTimerTask<'runtime, S, CAPACITY>, (S::RestoreError, Self)> {
        match self.storage.restore_modem_lp_timer_ready(self.owner) {
            Ok(()) => Ok(ControllerModemTimerTask::new(self.storage, self.runtime)),
            Err((error, owner)) => Err((
                error,
                Self {
                    storage: self.storage,
                    runtime: self.runtime,
                    owner,
                },
            )),
        }
    }
}
