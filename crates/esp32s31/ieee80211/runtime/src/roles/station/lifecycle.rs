#![expect(
    clippy::type_complexity,
    reason = "the lifecycle result preserves the concrete attempt owner and terminal frontier"
)]

use core::{fmt, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_sta::station::{
    StaAttemptFailure, StaFaultReason, StaLifecycleExit, StaLifecycleProgress, StaReconnectPolicy,
};
use oer_ieee80211_sta_service::station::StaLifecycleService;

use super::{
    StationAttemptRunner, StationCommand, StationCompletion, StationControlError,
    StationControlResources, StationController, backend::StationLifecycleBackend,
};

/// Stable application policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationConfiguration {
    reconnect: StaReconnectPolicy,
}

impl StationConfiguration {
    /// A normal application starts without a selected candidate and scans.
    pub const fn new(reconnect: StaReconnectPolicy) -> Self {
        Self { reconnect }
    }

    pub const fn reconnect_policy(self) -> StaReconnectPolicy {
        self.reconnect
    }
}

/// Exact role owner consumed when one station task is materialized.
pub struct StationStartResources<O> {
    owner: O,
}

impl<O> StationStartResources<O> {
    pub const fn new(owner: O) -> Self {
        Self { owner }
    }

    pub fn into_owner(self) -> O {
        self.owner
    }
}

/// Complete ownership frontier returned by a finite station task.
///
/// `owner` contains the exact phase/DMA/network resources returned by the
/// attempt. `runner` contains the platform integration resources which lived
/// beside that owner for the whole service, including an interrupt epoch when
/// the concrete composition owns one. Both are required before the physical
/// Wi-Fi owner can be reconstructed.
pub struct StationReturnedResources<O, R> {
    owner: O,
    runner: R,
}

/// Failed task materialization retaining every input which had not started.
///
/// A busy or poisoned command domain is not permission to drop the physical
/// station owner. The caller can keep this quarantined frontier or retry with a
/// different clean control resource without reacquiring PAC/DMA capabilities.
pub struct StationPrepareFailure<O, R> {
    pub error: StationControlError,
    config: StationConfiguration,
    resources: StationStartResources<O>,
    runner: R,
}

impl<O, R> StationPrepareFailure<O, R> {
    pub fn into_parts(
        self,
    ) -> (
        StationControlError,
        StationConfiguration,
        StationStartResources<O>,
        R,
    ) {
        (self.error, self.config, self.resources, self.runner)
    }
}

impl<O, R> fmt::Debug for StationPrepareFailure<O, R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StationPrepareFailure")
            .field("error", &self.error)
            .field("config", &self.config)
            .field("resources", &"<retained>")
            .field("runner", &"<retained>")
            .finish()
    }
}

impl<O, R> StationReturnedResources<O, R> {
    const fn new(owner: O, runner: R) -> Self {
        Self { owner, runner }
    }

    pub fn into_parts(self) -> (O, R) {
        (self.owner, self.runner)
    }

    /// Borrow the returned owner for value-only observation before consuming
    /// the complete reusable frontier.
    pub const fn owner(&self) -> &O {
        &self.owner
    }
}

/// Why a station runner returned through its normal stopped edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationStopReason {
    Requested(StationCommand),
    /// The backend stopped for a platform-specific reason without consuming a
    /// public controller command.
    Backend,
}

/// Complete application-visible result with the exact lifecycle owner.
pub enum StationExit<O, R, E, F = core::convert::Infallible> {
    Stopped {
        resources: StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        reason: StationStopReason,
    },
    RetryExhausted {
        resources: StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    Terminal {
        resources: StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    /// A non-reusable hardware frontier returned without fabricating `O`.
    /// The platform runner is retained separately because it may own observers
    /// or integration capabilities needed to diagnose the fault.
    Faulted {
        fault: F,
        reason: StaFaultReason,
        runner: R,
        progress: StaLifecycleProgress,
    },
}

/// Materialize one task-owned station lifecycle and its hardware-free
/// application controller in a single transaction.
pub fn prepare_esp32s31_station_task<'control, M, R>(
    config: StationConfiguration,
    resources: StationStartResources<R::Owner>,
    control: &'control StationControlResources<M>,
    runner: R,
) -> Result<
    (StationController<'control, M>, StationTask<'control, M, R>),
    StationPrepareFailure<R::Owner, R>,
>
where
    M: RawMutex,
    R: StationAttemptRunner<M>,
{
    let (controller, receiver) = match control.split() {
        Ok(endpoints) => endpoints,
        Err(error) => {
            return Err(StationPrepareFailure {
                error,
                config,
                resources,
                runner,
            });
        }
    };
    let backend = StationLifecycleBackend::new(receiver, runner);
    let task = StationTask {
        lifecycle: Some(StaLifecycleService::new(backend, config.reconnect)),
        resources: Some(resources),
        control: controller.resources(),
        completion_pending: true,
        _mutex: PhantomData,
    };
    Ok((controller, task))
}

/// Owned station service. Running it consumes the initial owner and always
/// returns both the lifecycle's exact final owner and its platform runner in
/// [`StationExit`].
///
/// Dropping this task or its in-flight [`run`](Self::run) future is not a
/// shutdown operation. It publishes [`StationCompletion::Faulted`]
/// and permanently prevents another task from splitting the same control
/// resources. Live lower-level owners independently retain or poison their
/// DMA/IRQ resources, so cancellation cannot be mistaken for quiescence.
// CAPABILITY: station-lifecycle-owners, wifi-interfaces-and-operating-modes-infrastructure-station
pub struct StationTask<'control, M: RawMutex, R: StationAttemptRunner<M>> {
    lifecycle: Option<StaLifecycleService<StationLifecycleBackend<'control, M, R>>>,
    resources: Option<StationStartResources<R::Owner>>,
    control: &'control StationControlResources<M>,
    completion_pending: bool,
    _mutex: PhantomData<M>,
}

impl<M, R> StationTask<'_, M, R>
where
    M: RawMutex,
    R: StationAttemptRunner<M>,
{
    /// Run the task in its existing owner-held storage.
    ///
    /// The task contains the complete station owner graph. Borrowing it here
    /// is intentional: consuming `self` would make async lowering construct
    /// and move another copy-sized future through the live CPU stack before
    /// it reaches its already-static Embassy parent task. Every owned field is
    /// still taken exactly once, and [`Drop`] remains fail-closed until the
    /// terminal completion has been published.
    pub async fn run(&mut self) -> StationExit<R::Owner, R, R::Error, R::Fault> {
        let owner = self
            .resources
            .take()
            .expect("station task starts with exactly one owner")
            .into_owner();
        let mut lifecycle = self
            .lifecycle
            .take()
            .expect("station task starts with exactly one lifecycle");
        let outcome = await_stack_boundary!(lifecycle.run(owner));
        let (receiver, runner) = lifecycle.into_backend().into_parts();
        let (exit, completion) = match outcome {
            StaLifecycleExit::Stopped { owner, progress } => (
                StationExit::Stopped {
                    resources: StationReturnedResources::new(owner, runner),
                    progress,
                    reason: self
                        .control
                        .take_terminal()
                        .map_or(StationStopReason::Backend, |command| {
                            StationStopReason::Requested(command)
                        }),
                },
                StationCompletion::Stopped,
            ),
            StaLifecycleExit::Exhausted {
                owner,
                progress,
                failure,
            } => (
                StationExit::RetryExhausted {
                    resources: StationReturnedResources::new(owner, runner),
                    progress,
                    failure,
                },
                StationCompletion::Ended,
            ),
            StaLifecycleExit::Terminal {
                owner,
                progress,
                failure,
            } => (
                StationExit::Terminal {
                    resources: StationReturnedResources::new(owner, runner),
                    progress,
                    failure,
                },
                StationCompletion::Ended,
            ),
            StaLifecycleExit::Faulted {
                fault,
                reason,
                progress,
            } => (
                StationExit::Faulted {
                    fault,
                    reason,
                    runner,
                    progress,
                },
                StationCompletion::Faulted,
            ),
        };
        self.control.publish_completion(completion);
        self.completion_pending = false;
        // The receiver's fail-closed Drop observes the completion published
        // above. Keep it alive until this edge so extracting the runner cannot
        // transiently classify a clean stop as cancellation.
        drop(receiver);
        exit
    }
}

impl<M, R> Drop for StationTask<'_, M, R>
where
    M: RawMutex,
    R: StationAttemptRunner<M>,
{
    fn drop(&mut self) {
        if self.completion_pending {
            // Active phase owners have their own fail-closed Drop contracts.
            // This acknowledgement never claims that IRQ or DMA stopped; it
            // only prevents the application from waiting forever and makes
            // the non-reusable fault frontier explicit.
            self.control.publish_completion(StationCompletion::Faulted);
        }
    }
}
