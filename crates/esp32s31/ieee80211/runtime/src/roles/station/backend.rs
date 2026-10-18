#![expect(
    clippy::manual_async_fn,
    reason = "station backend implementations preserve explicit borrowed Future contracts"
)]

//! Driver-owned command and retry boundary around concrete STA phases.

use core::future::Future;

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::Duration;

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_sta::station::{
    StaAttemptContext, StaAttemptOutcome, StaBackoffOutcome, StaBackoffReason, StaFault,
    StaLifecycleBackend,
};

use super::{StationCommand, StationCommandReceiver};

/// Concrete scan/join/connected phase implementation used by one station
/// task.
///
/// Command priority, terminal acknowledgement and bounded reconnect backoff
/// remain in the driver-owned backend below. A board or HIL implementation
/// supplies only the actual phase transaction and optional observations.
///
/// # Lifecycle contract
///
/// `Owner` is the unique owner frontier for every DMA ring, interrupt route,
/// TX epoch and spawned protocol task used by the attempt. Every returned
/// outcome must contain that same frontier after all phase-local borrows have
/// ended. In particular, [`StaAttemptOutcome::Stopped`] is permitted only
/// after the runner has proved its IRQ and DMA actors quiescent; observing a
/// stop command is not that proof.
///
/// The future returned by [`run_attempt`](Self::run_attempt) may be cancelled
/// when its enclosing station task is destroyed. Any live hardware token
/// dropped on that path must therefore fail closed and poison its reusable
/// arena or route. A runner must never use `Drop` to claim a clean stop or
/// make static DMA/ISR storage reusable. Recovery remains a supervisor policy.
pub trait StationAttemptRunner<M: RawMutex> {
    type Owner;
    type Error;
    type Fault: StaFault;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + 'a;

    fn command_deferred(&mut self, _command: StationCommand, _accepted: bool) {}

    fn backoff_started(&mut self, _delay_millis: u32, _reason: StaBackoffReason) {}

    /// The clock reconnect backoff waits on.
    type Timer: oer_time::Timer;

    fn timer(&self) -> &Self::Timer;
}

pub(super) struct StationLifecycleBackend<'control, M: RawMutex, R: StationAttemptRunner<M>> {
    control: StationCommandReceiver<'control, M>,
    runner: R,
}

impl<'control, M, R> StationLifecycleBackend<'control, M, R>
where
    M: RawMutex,
    R: StationAttemptRunner<M>,
{
    pub(super) const fn new(control: StationCommandReceiver<'control, M>, runner: R) -> Self {
        Self { control, runner }
    }

    /// Return the exact platform runner after the lifecycle has reached a
    /// finite edge. The runner may own role-external resources such as the
    /// installed interrupt epoch, so dropping it would make physical-radio
    /// dematerialization impossible.
    pub(super) fn into_parts(self) -> (StationCommandReceiver<'control, M>, R) {
        (self.control, self.runner)
    }
}

impl<M, R> StaLifecycleBackend for StationLifecycleBackend<'_, M, R>
where
    M: RawMutex,
    R: StationAttemptRunner<M>,
{
    type Owner = R::Owner;
    type Error = R::Error;
    type Fault = R::Fault;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + '_ {
        async move {
            if let Some(command) = self.control.try_take() {
                match command {
                    StationCommand::Reconnect => {
                        let accepted = self.control.defer(command);
                        self.runner.command_deferred(command, accepted);
                    }
                    StationCommand::Disconnect | StationCommand::Stop => {
                        self.control.record_terminal(command);
                        return StaAttemptOutcome::Stopped { owner };
                    }
                }
            }
            await_stack_boundary!(self.runner.run_attempt(owner, context, &mut self.control))
        }
    }

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
        async move {
            self.runner.backoff_started(delay_millis, reason);
            match select(
                crate::time::wait_for(self.runner.timer(), Duration::from_millis(delay_millis)),
                self.control.wait(),
            )
            .await
            {
                Either::First(()) => StaBackoffOutcome::Elapsed { owner },
                Either::Second(command @ StationCommand::Reconnect) => {
                    let accepted = self.control.defer(command);
                    self.runner.command_deferred(command, accepted);
                    StaBackoffOutcome::Elapsed { owner }
                }
                Either::Second(command @ (StationCommand::Disconnect | StationCommand::Stop)) => {
                    self.control.record_terminal(command);
                    StaBackoffOutcome::Stopped { owner }
                }
            }
        }
    }
}
