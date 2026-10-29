//! Chip-independent finite shutdown for tasks attached to one epoch.
//!
//! A radio runner can return while an executor task still owns protocol
//! scratch or queue leases. This module provides a split ownership contract:
//! the parent receives only the stop/wait capability, while the task receives
//! only the request/return capability. A dropped task endpoint poisons the
//! resources, so a replacement epoch cannot erase an unreturned owner.
//! Waiting only observes readiness: each controller retains its returned owner
//! outside the observation future. `try_finish(self)` consumes the capability
//! and returns that owner once. A timeout or cancelled wait leaves the same
//! group available for another observation, including after one child stops.

use core::{
    future::{Future, poll_fn},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
    task::Poll,
};

use crate::{Duration, Timer};
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use oer_memory::ClaimCell;
use oer_sync::PriorityWaker;

const IDLE: u8 = 0;
const ACTIVE: u8 = 1;
const POISONED: u8 = 2;

/// Executor tasks which borrow resources from one connected Wi-Fi epoch.
pub trait ConnectedTaskGroup {
    type Stopped;

    /// Publish the idempotent cooperative stop request to every task.
    fn request_stop(&mut self);

    /// Observe completion without transferring any owner out of this group.
    /// Cancellation must retain partial results in `self`, not in this future.
    #[allow(async_fn_in_trait)]
    async fn wait_stopped(&mut self);

    /// Consume a completed group, or return the entire still-stopping group.
    /// This may be attempted without first waiting; readiness alone never
    /// releases a resource or permits reuse of its control storage.
    fn try_finish(self) -> Result<Self::Stopped, Self>
    where
        Self: Sized;
}

/// One owner-bearing connected task plus an auxiliary task which must stop
/// before the epoch can release its radio resources.
///
/// The primary task returns the owner needed by the next driver epoch. The
/// auxiliary task may return bookkeeping of its own, but that value is kept
/// private: completion of both endpoints is required before the primary owner
/// is revealed. This is useful for application or test services which share
/// an Embassy network epoch without giving those services access to DMA or
/// PAC ownership.
pub struct ConnectedTaskGroupWithAuxiliary<P: ConnectedTaskGroup, A: ConnectedTaskGroup> {
    primary: TaskCompletion<P>,
    auxiliary: TaskCompletion<A>,
}

/// An early consuming finish may return one child before the other. The group
/// retains that result, just as a child retains results of a cancelled wait.
enum TaskCompletion<G: ConnectedTaskGroup> {
    Waiting(G),
    Finished(G::Stopped),
}

impl<G: ConnectedTaskGroup> TaskCompletion<G> {
    fn request_stop(&mut self) {
        if let Self::Waiting(group) = self {
            group.request_stop();
        }
    }

    async fn wait_stopped(&mut self) {
        if let Self::Waiting(group) = self {
            group.wait_stopped().await;
        }
    }

    fn finish(self) -> Self {
        match self {
            Self::Waiting(group) => match group.try_finish() {
                Ok(stopped) => Self::Finished(stopped),
                Err(group) => Self::Waiting(group),
            },
            finished => finished,
        }
    }
}

impl<P: ConnectedTaskGroup, A: ConnectedTaskGroup> ConnectedTaskGroupWithAuxiliary<P, A> {
    pub const fn new(primary: P, auxiliary: A) -> Self {
        Self {
            primary: TaskCompletion::Waiting(primary),
            auxiliary: TaskCompletion::Waiting(auxiliary),
        }
    }
}

impl<P, A> ConnectedTaskGroup for ConnectedTaskGroupWithAuxiliary<P, A>
where
    P: ConnectedTaskGroup,
    A: ConnectedTaskGroup,
{
    type Stopped = P::Stopped;

    fn request_stop(&mut self) {
        self.primary.request_stop();
        self.auxiliary.request_stop();
    }

    async fn wait_stopped(&mut self) {
        join(self.primary.wait_stopped(), self.auxiliary.wait_stopped()).await;
    }

    fn try_finish(self) -> Result<Self::Stopped, Self> {
        match (self.primary.finish(), self.auxiliary.finish()) {
            (TaskCompletion::Finished(primary), TaskCompletion::Finished(_auxiliary)) => {
                Ok(primary)
            }
            (primary, auxiliary) => Err(Self { primary, auxiliary }),
        }
    }
}

/// Why task-control resources cannot create another epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedTaskControlError {
    Active,
    Poisoned,
}

/// Reusable lock-free storage for one sequential executor task on one hart.
///
/// Only a completed task endpoint followed by consumption of its returned
/// owner restores `Idle`. Retirement clears that epoch's stop signal before
/// publishing `Idle`, including stops issued after the endpoint completed.
/// There is deliberately no safe reset operation.
pub struct ConnectedTaskControlResources<S> {
    stop: AtomicBool,
    stop_waker: PriorityWaker,
    stopped: ClaimCell<Option<S>>,
    stopped_waker: PriorityWaker,
    state: AtomicU8,
    initialized: AtomicBool,
}

pub type ConnectedTaskEndpoints<'resources, S> = (
    ConnectedTaskController<'resources, S>,
    ConnectedTaskEndpoint<'resources, S>,
);

/// A task-control epoch reserved before any driver owner is decomposed.
///
/// Keeping both endpoints together permits an infallible rollback while no
/// executor task has observed either capability. Once `into_endpoints` is
/// called, ordinary controller/endpoint shutdown rules apply.
pub struct ConnectedTaskReservation<'resources, S> {
    controller: ConnectedTaskController<'resources, S>,
    endpoint: ConnectedTaskEndpoint<'resources, S>,
}

impl<'resources, S> ConnectedTaskReservation<'resources, S> {
    pub fn into_endpoints(self) -> ConnectedTaskEndpoints<'resources, S> {
        (self.controller, self.endpoint)
    }

    /// Roll back a reservation which was never exposed to an executor task.
    pub fn abort_unused(mut self) {
        self.controller.completed = true;
        self.endpoint.completed = true;
        self.controller
            .resources
            .state
            .store(IDLE, Ordering::Release);
    }
}

impl<S> ConnectedTaskControlResources<S> {
    pub const fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            stop_waker: PriorityWaker::new(),
            stopped: ClaimCell::new(),
            stopped_waker: PriorityWaker::new(),
            state: AtomicU8::new(IDLE),
            initialized: AtomicBool::new(false),
        }
    }

    /// Create exactly one parent/task capability pair for a fresh epoch.
    pub fn split(&self) -> Result<ConnectedTaskEndpoints<'_, S>, ConnectedTaskControlError> {
        match self
            .state
            .compare_exchange(IDLE, ACTIVE, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                if !self.initialized.swap(true, Ordering::AcqRel)
                    && self.stopped.install(None).is_err()
                {
                    self.state.store(POISONED, Ordering::Release);
                    return Err(ConnectedTaskControlError::Poisoned);
                }
                Ok((
                    ConnectedTaskController {
                        resources: self,
                        completed: false,
                        stopped: None,
                    },
                    ConnectedTaskEndpoint {
                        resources: self,
                        completed: false,
                    },
                ))
            }
            Err(ACTIVE) => Err(ConnectedTaskControlError::Active),
            Err(_) => Err(ConnectedTaskControlError::Poisoned),
        }
    }

    /// Reserve both sides of a future task epoch before moving any related
    /// DMA, IRQ or protocol owner.
    pub fn reserve(&self) -> Result<ConnectedTaskReservation<'_, S>, ConnectedTaskControlError> {
        let (controller, endpoint) = self.split()?;
        Ok(ConnectedTaskReservation {
            controller,
            endpoint,
        })
    }

    pub fn is_poisoned(&self) -> bool {
        self.state.load(Ordering::Acquire) == POISONED
    }

    /// Abandon an active pair without overwriting a terminal state already
    /// published by the other endpoint.
    fn poison_active(&self) {
        match self
            .state
            .compare_exchange(ACTIVE, POISONED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(_) => {}
        }
    }
}

impl<S> Default for ConnectedTaskControlResources<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Parent-side capability: request stop and recover the returned owner.
pub struct ConnectedTaskController<'resources, S> {
    resources: &'resources ConnectedTaskControlResources<S>,
    completed: bool,
    /// Returned ownership lives here across every cancelled observation.
    stopped: Option<S>,
}

impl<S> ConnectedTaskGroup for ConnectedTaskController<'_, S> {
    type Stopped = S;

    fn request_stop(&mut self) {
        if !self.completed {
            self.resources.stop.store(true, Ordering::Release);
            self.resources.stop_waker.wake();
        }
    }

    async fn wait_stopped(&mut self) {
        if self.stopped.is_none() {
            self.stopped = Some(
                poll_fn(|cx| {
                    self.resources.stopped_waker.register(cx.waker());
                    match self.resources.stopped.lend(Option::take) {
                        Ok(Some(owner)) => Poll::Ready(owner),
                        Ok(None) | Err(_) => Poll::Pending,
                    }
                })
                .await,
            );
        }
    }

    fn try_finish(mut self) -> Result<Self::Stopped, Self> {
        let Some(stopped) = self.stopped.take() else {
            return Err(self);
        };
        // Only `ConnectedTaskEndpoint::complete` can publish this value.
        // The endpoint has been consumed and its destructor disarmed.
        // This controller is consumed too; observation alone kept it active.
        self.completed = true;
        self.resources.stop.store(false, Ordering::Release);
        self.resources.state.store(IDLE, Ordering::Release);
        Ok(stopped)
    }
}

impl<S> Drop for ConnectedTaskController<'_, S> {
    fn drop(&mut self) {
        if !self.completed {
            self.resources.poison_active();
        }
    }
}

/// Task-side capability: observe stop and return the exact stopped owner.
pub struct ConnectedTaskEndpoint<'resources, S> {
    resources: &'resources ConnectedTaskControlResources<S>,
    completed: bool,
}

impl<S> ConnectedTaskEndpoint<'_, S> {
    pub fn wait_for_stop(&self) -> impl Future<Output = ()> + '_ {
        poll_fn(|cx| {
            self.resources.stop_waker.register(cx.waker());
            if self.resources.stop.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }

    /// Consume the task endpoint and publish its exact stopped owner.
    pub fn complete(mut self, stopped: S) -> Result<(), S> {
        let mut owner = Some(stopped);
        let _publication = self.resources.stopped.lend(|slot| {
            if slot.is_none() {
                *slot = owner.take();
            }
        });
        match owner {
            Some(owner) => Err(owner),
            None => {
                self.completed = true;
                self.resources.stopped_waker.wake();
                Ok(())
            }
        }
    }
}

impl<S> Drop for ConnectedTaskEndpoint<'_, S> {
    fn drop(&mut self) {
        if !self.completed {
            self.resources.poison_active();
        }
    }
}

/// Result of one deadline-bounded observation of task shutdown.
///
/// `Pending` does not poison the task group and does not imply that radio
/// reset is required. The caller may keep waiting or observe the same stopping
/// transaction again; the task owners remain inside the group until
/// [`ConnectedTaskGroup::try_finish`] consumes the group and returns them.
pub enum ConnectedTaskStopAttempt {
    /// Every child has returned, with owners still retained in the group.
    Stopped,
    Pending {
        waited: Duration,
    },
}

impl ConnectedTaskStopAttempt {
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// Request and await release of all executor tasks attached to one epoch.
///
/// This observation has no internal deadline. On return the group retains all
/// stopped owners until its consuming `try_finish`; cancellation retains the
/// group and every partial result as well.
pub async fn stop_connected_task_group<G>(group: &mut G)
where
    G: ConnectedTaskGroup,
{
    group.request_stop();
    group.wait_stopped().await
}

/// Request task shutdown and observe it for at most `timeout` on `timer`.
///
/// The deadline covers the complete group, not each task independently.
/// Expiration is only a liveness observation; it never converts a still-owned
/// task graph into a quarantined hardware frontier. A deadline past the
/// timer's range never expires.
pub async fn stop_connected_task_group_until<G>(
    group: &mut G,
    timer: &impl Timer,
    timeout: Duration,
) -> ConnectedTaskStopAttempt
where
    G: ConnectedTaskGroup,
{
    group.request_stop();
    let expiry = async {
        if timer.wait_for(timeout).await.is_err() {
            core::future::pending::<()>().await;
        }
    };
    match select(group.wait_stopped(), expiry).await {
        Either::First(()) => ConnectedTaskStopAttempt::Stopped,
        Either::Second(()) => ConnectedTaskStopAttempt::Pending { waited: timeout },
    }
}
