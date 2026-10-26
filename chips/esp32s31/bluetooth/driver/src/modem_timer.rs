//! Source-127 timer ownership, readiness and finite task transitions.
//!
//! Boot supplies the timer runtime once. This module retains the queue and HAL
//! snapshot across task waits and exchanges plans with the timer's gate in the
//! Bluetooth interrupt handler ([`crate::interrupt::ModemLpTimerPort`]); it
//! does not own Controller scheduling or HCI command dispatch.

use crate::{
    modem_lp_timer_queue::{
        ModemLpTimerEventCell, ModemLpTimerEventPublication, ModemLpTimerExpiration,
        ModemLpTimerExpirationState, ModemLpTimerSoftwareState, ModemLpTimerSoftwareStateStep,
    },
    runtime_resources::ControllerModemTimerRuntime,
};
use oer_esp32s31_hal::bluetooth::{ModemLpTimerPlan, ModemLpTimerWork};
use oer_interrupt_table::signal::{ForeignContext, SignalExecutor};

/// Value-only snapshot/plan boundary for the source-127 task.
///
/// The timer's gate implements it ([`crate::interrupt::ModemLpTimerPort`]).
/// Register authority stays in the handler throughout queue processing.
/// Submitting a plan does no MMIO; the handler alone applies and arms it.
pub trait ModemLpTimerPlanStorage {
    /// Exact reason task context could not acquire pending work.
    type TakeError;
    /// Exact reason the unarmed plan could not return to ISR storage.
    type SubmitError;

    /// Take the value-only work snapshot published by the handler.
    fn take_modem_lp_timer_work(&self) -> Result<ModemLpTimerWork, Self::TakeError>;

    /// Submit the unarmed plan, retaining it on rejection.
    fn submit_modem_lp_timer_plan(
        &self,
        owner: ModemLpTimerPlan,
    ) -> Result<(), (Self::SubmitError, ModemLpTimerPlan)>;
}

/// Why the timer's gate refused a software exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemLpTimerPortError {
    /// The handler handed on no work snapshot.
    NoPendingWork,
    /// The handler has not taken the previous plan.
    Occupied,
}

impl<R: crate::interrupt::RadioDoorbell> ModemLpTimerPlanStorage
    for crate::interrupt::ModemLpTimerPort<crate::interrupt::ChipGate, R>
{
    type TakeError = ModemLpTimerPortError;
    type SubmitError = ModemLpTimerPortError;

    fn take_modem_lp_timer_work(&self) -> Result<ModemLpTimerWork, ModemLpTimerPortError> {
        self.take_work().ok_or(ModemLpTimerPortError::NoPendingWork)
    }

    fn submit_modem_lp_timer_plan(
        &self,
        owner: ModemLpTimerPlan,
    ) -> Result<(), (ModemLpTimerPortError, ModemLpTimerPlan)> {
        self.submit_plan(owner)
            .map_err(|owner| (ModemLpTimerPortError::Occupied, owner))
    }
}

/// Stable state retained inside the disjoint source-127 task endpoint.
enum ControllerModemTimerTaskPhase {
    Idle,
    Work(ModemLpTimerSoftwareState),
    Expiration(ModemLpTimerExpirationState),
    Plan(ModemLpTimerPlan),
}

/// Borrowed readiness class for the source-127 task endpoint.
///
/// The value carries no timer owner and may be held by an executor wait future.
/// The queue and snapshot epoch remain inside
/// [`ControllerModemTimerTask`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerModemTimerReadinessClass {
    /// No interrupt-owned software work is known to be pending.
    Interrupt,
    /// One finite software transition can run immediately.
    Step,
    /// Expiration publication is waiting for the one-event cell to become empty.
    EventCapacity,
    /// An unarmed plan is ready for submission to the handler.
    Plan,
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
            ControllerModemTimerReadinessClass::Step | ControllerModemTimerReadinessClass::Plan => {
                true
            }
            ControllerModemTimerReadinessClass::EventCapacity => !self.events.is_pending(),
        })
    }
}

/// Result of acquiring one durable source-127 software-work snapshot.
#[must_use = "a begin result must be handled without losing stable readiness"]
pub enum ControllerModemTimerBegin<E> {
    /// No durable interrupt publication currently requests task work.
    NotReady,
    /// The snapshot entered the endpoint and one finite software step is ready.
    Started,
    /// Stable storage rejected acquisition; the endpoint remains idle.
    StorageRejected(E),
    /// A software or plan phase is already retained by this endpoint.
    AlreadyActive,
    /// The task runs outside its worker wake's executor; nothing changed.
    ForeignExecutor,
}

/// Result of one finite source-127 task transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the next source-127 readiness class must be observed"]
pub enum ControllerModemTimerStep {
    /// No software snapshot has been acquired.
    Idle,
    /// One expiration now owns the durable publication edge.
    ExpirationPending(ModemLpTimerExpiration),
    /// The expiration was published and later work remains retained.
    Published(ModemLpTimerEventPublication),
    /// The expiration cell is occupied and the unchanged publication remains retained.
    Backpressured(ModemLpTimerExpiration),
    /// Queue processing produced an unarmed plan retained inside the endpoint.
    PlanPending,
}

/// Result of submitting an unarmed plan to the handler.
#[must_use = "a rejected plan remains retained by the timer task"]
pub enum ControllerModemTimerPlanSubmission<E> {
    /// The unarmed plan is queued for application by the handler.
    Submitted,
    /// Stable storage rejected submission; the exact unarmed plan remains private.
    StorageRejected(E),
    /// No unarmed plan is currently retained.
    NotReady,
    /// The task runs outside its worker wake's executor; the plan stays
    /// retained.
    ForeignExecutor,
}

/// Disjoint task-context source-127 owner for one final Controller runtime.
///
/// This endpoint owns the mutable queue and a copy of the handler's snapshot epoch.
/// Stable storage is shared only as the platform exchange boundary with the
/// interrupt service. Value-only work remains private across borrowed
/// readiness and finite `begin`, `step`, and `submit_plan` calls, so an executor
/// future never needs to own them.
#[must_use = "the modem timer task retains source-127 queue and unarmed plans"]
pub struct ControllerModemTimerTask<'runtime, S, const CAPACITY: usize, X: SignalExecutor> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY, X>,
    phase: ControllerModemTimerTaskPhase,
}

impl<'runtime, S, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerTask<'runtime, S, CAPACITY, X>
where
    S: ModemLpTimerPlanStorage,
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
            ControllerModemTimerTaskPhase::Plan(_) => ControllerModemTimerReadinessClass::Plan,
        };
        ControllerModemTimerReadiness {
            class,
            worker_wake: self.runtime.worker_wake(),
            events: self.runtime.events(),
        }
    }

    /// Acquire exactly one software-work snapshot after borrowed readiness.
    pub fn begin(&mut self) -> ControllerModemTimerBegin<S::TakeError> {
        if !matches!(self.phase, ControllerModemTimerTaskPhase::Idle) {
            return ControllerModemTimerBegin::AlreadyActive;
        }
        match self.runtime.worker_wake().is_pending() {
            Ok(true) => {}
            Ok(false) => return ControllerModemTimerBegin::NotReady,
            Err(ForeignContext) => return ControllerModemTimerBegin::ForeignExecutor,
        }
        match self.storage.take_modem_lp_timer_work() {
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

    /// Advance exactly one value-only queue, publication or plan transition.
    pub fn step(&mut self) -> ControllerModemTimerStep {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        match phase {
            ControllerModemTimerTaskPhase::Idle => ControllerModemTimerStep::Idle,
            ControllerModemTimerTaskPhase::Work(work) => match work.step(&mut self.runtime.queue) {
                ModemLpTimerSoftwareStateStep::Expiration(pending) => {
                    let event = pending.event();
                    self.phase = ControllerModemTimerTaskPhase::Expiration(pending);
                    ControllerModemTimerStep::ExpirationPending(event)
                }
                ModemLpTimerSoftwareStateStep::Planned(owner) => {
                    self.phase = ControllerModemTimerTaskPhase::Plan(owner);
                    ControllerModemTimerStep::PlanPending
                }
            },
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
            ControllerModemTimerTaskPhase::Plan(owner) => {
                self.phase = ControllerModemTimerTaskPhase::Plan(owner);
                ControllerModemTimerStep::PlanPending
            }
        }
    }

    /// Submit one unarmed plan to the handler; hardware is armed there later.
    pub fn submit_plan(&mut self) -> ControllerModemTimerPlanSubmission<S::SubmitError> {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        let ControllerModemTimerTaskPhase::Plan(owner) = phase else {
            self.phase = phase;
            return ControllerModemTimerPlanSubmission::NotReady;
        };
        if let Err(ForeignContext) = self.runtime.worker_wake.take() {
            self.phase = ControllerModemTimerTaskPhase::Plan(owner);
            return ControllerModemTimerPlanSubmission::ForeignExecutor;
        }
        match self.storage.submit_modem_lp_timer_plan(owner) {
            Ok(()) => ControllerModemTimerPlanSubmission::Submitted,
            Err((error, owner)) => {
                self.phase = ControllerModemTimerTaskPhase::Plan(owner);
                ControllerModemTimerPlanSubmission::StorageRejected(error)
            }
        }
    }

    /// Consume one durably published expiration, if present.
    pub fn take_expiration(&mut self) -> Option<ModemLpTimerExpiration> {
        self.runtime.events().take()
    }
}

/// Drained software timer task; its HAL owner stays with the timer's
/// interrupt handler, which stops the counter and both measured lanes and keeps the owner until
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

impl<'runtime, S: ModemLpTimerPlanStorage, const CAPACITY: usize, X: SignalExecutor>
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

impl<'runtime, S: ModemLpTimerPlanStorage, const CAPACITY: usize, X: SignalExecutor>
    ControllerModemTimerRetired<'runtime, S, CAPACITY, X>
{
    /// Resume the same software epoch; its owner never left the handler.
    pub fn resume(self) -> ControllerModemTimerTask<'runtime, S, CAPACITY, X> {
        ControllerModemTimerTask::new(self.storage, self.runtime)
    }
}
