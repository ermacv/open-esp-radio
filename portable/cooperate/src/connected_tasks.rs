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
    future::Future,
    sync::atomic::{AtomicU8, Ordering},
};

use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use oer_time::{Duration, Timer};

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

/// Reusable signal storage for one sequential executor task.
///
/// Only a completed task endpoint followed by consumption of its returned
/// owner restores `Idle`. Retirement clears that epoch's stop signal before
/// publishing `Idle`, including stops issued after the endpoint completed.
/// There is deliberately no safe reset operation.
pub struct ConnectedTaskControlResources<M: RawMutex, S> {
    stop: Signal<M, ()>,
    stopped: Signal<M, S>,
    state: AtomicU8,
}

pub type ConnectedTaskEndpoints<'resources, M, S> = (
    ConnectedTaskController<'resources, M, S>,
    ConnectedTaskEndpoint<'resources, M, S>,
);

/// A task-control epoch reserved before any driver owner is decomposed.
///
/// Keeping both endpoints together permits an infallible rollback while no
/// executor task has observed either capability. Once `into_endpoints` is
/// called, ordinary controller/endpoint shutdown rules apply.
pub struct ConnectedTaskReservation<'resources, M: RawMutex, S> {
    controller: ConnectedTaskController<'resources, M, S>,
    endpoint: ConnectedTaskEndpoint<'resources, M, S>,
}

impl<'resources, M: RawMutex, S> ConnectedTaskReservation<'resources, M, S> {
    pub fn into_endpoints(self) -> ConnectedTaskEndpoints<'resources, M, S> {
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

impl<M: RawMutex, S> ConnectedTaskControlResources<M, S> {
    pub const fn new() -> Self {
        Self {
            stop: Signal::new(),
            stopped: Signal::new(),
            state: AtomicU8::new(IDLE),
        }
    }

    /// Create exactly one parent/task capability pair for a fresh epoch.
    pub fn split(&self) -> Result<ConnectedTaskEndpoints<'_, M, S>, ConnectedTaskControlError> {
        match self
            .state
            .compare_exchange(IDLE, ACTIVE, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok((
                ConnectedTaskController {
                    resources: self,
                    completed: false,
                    stopped: None,
                },
                ConnectedTaskEndpoint {
                    resources: self,
                    completed: false,
                },
            )),
            Err(ACTIVE) => Err(ConnectedTaskControlError::Active),
            Err(_) => Err(ConnectedTaskControlError::Poisoned),
        }
    }

    /// Reserve both sides of a future task epoch before moving any related
    /// DMA, IRQ or protocol owner.
    pub fn reserve(&self) -> Result<ConnectedTaskReservation<'_, M, S>, ConnectedTaskControlError> {
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

impl<M: RawMutex, S> Default for ConnectedTaskControlResources<M, S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Parent-side capability: request stop and recover the returned owner.
pub struct ConnectedTaskController<'resources, M: RawMutex, S> {
    resources: &'resources ConnectedTaskControlResources<M, S>,
    completed: bool,
    /// Returned ownership lives here across every cancelled observation.
    stopped: Option<S>,
}

impl<M: RawMutex, S> ConnectedTaskGroup for ConnectedTaskController<'_, M, S> {
    type Stopped = S;

    fn request_stop(&mut self) {
        if !self.completed {
            self.resources.stop.signal(());
        }
    }

    async fn wait_stopped(&mut self) {
        if self.stopped.is_none() {
            self.stopped = Some(self.resources.stopped.wait().await);
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
        self.resources.stop.reset();
        self.resources.state.store(IDLE, Ordering::Release);
        Ok(stopped)
    }
}

impl<M: RawMutex, S> Drop for ConnectedTaskController<'_, M, S> {
    fn drop(&mut self) {
        if !self.completed {
            self.resources.poison_active();
        }
    }
}

/// Task-side capability: observe stop and return the exact stopped owner.
pub struct ConnectedTaskEndpoint<'resources, M: RawMutex, S> {
    resources: &'resources ConnectedTaskControlResources<M, S>,
    completed: bool,
}

impl<M: RawMutex, S> ConnectedTaskEndpoint<'_, M, S> {
    pub fn wait_for_stop(&self) -> impl Future<Output = ()> + '_ {
        self.resources.stop.wait()
    }

    /// Consume the task endpoint and publish its exact stopped owner.
    pub fn complete(mut self, stopped: S) {
        self.completed = true;
        self.resources.stopped.signal(stopped);
    }
}

impl<M: RawMutex, S> Drop for ConnectedTaskEndpoint<'_, M, S> {
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

#[cfg(test)]
mod tests;
