//! Allocation-free station attempt and reconnect orchestration.
//!
//! The values, reconnect policy and [`StaLifecycleBackend`] port live in
//! `oer_ieee80211_sta::station`; this module drives them.

use core::{
    future::{Future, poll_fn},
    pin::{Pin, pin},
    task::{Context, Poll},
};

use oer_ieee80211_sta::station::{
    StaAttemptContext, StaAttemptOutcome, StaBackoffOutcome, StaBackoffReason,
    StaFailureDisposition, StaFault, StaLifecycleBackend, StaLifecycleExit, StaLifecycleProgress,
    StaNextCandidate, StaReconnectPolicy,
};

/// Poll a child state machine through a real call boundary.
///
/// The future remains stored in its parent future. Only the CPU stack used by
/// its `poll` implementation is isolated, preventing fat LTO from merging a
/// complete MLME attempt into the outer retry loop.
async fn poll_with_stack_boundary<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut output = None;
    poll_fn(|context| poll_pinned_future(future.as_mut(), &mut output, context)).await;
    output.expect("completed stack boundary stores its output")
}

#[inline(never)]
fn poll_pinned_future<F: Future>(
    future: Pin<&mut F>,
    output: &mut Option<F::Output>,
    context: &mut Context<'_>,
) -> Poll<()> {
    type PollFn<F> = for<'future, 'context, 'wake> fn(
        Pin<&'future mut F>,
        &'context mut Context<'wake>,
    ) -> Poll<<F as Future>::Output>;
    let poll: PollFn<F> = F::poll;
    match core::hint::black_box(poll)(future, context) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(value) => {
            *output = Some(value);
            Poll::Ready(())
        }
    }
}

/// Outer allocation-free station lifecycle owner.
pub struct StaLifecycleService<B> {
    backend: B,
    policy: StaReconnectPolicy,
}

impl<B> StaLifecycleService<B>
where
    B: StaLifecycleBackend,
{
    pub const fn new(backend: B, policy: StaReconnectPolicy) -> Self {
        Self { backend, policy }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    /// Run until caller stop, retry exhaustion or a terminal failure.
    pub async fn run(&mut self, owner: B::Owner) -> StaLifecycleExit<B::Owner, B::Error, B::Fault> {
        let mut owner = owner;
        let mut progress = StaLifecycleProgress::default();
        let mut generation_attempt = 1_u16;
        // A cold service never accepts a caller-proven candidate. Candidate
        // reuse is legal only after this lifecycle itself returns a connected
        // epoch with an explicit `StaNextCandidate::Reuse` disposition.
        let mut refresh_candidate = true;
        loop {
            progress.attempts_started = progress.attempts_started.saturating_add(1);
            progress.final_generation_attempt = generation_attempt;
            let context = StaAttemptContext {
                generation: progress.connected_epochs,
                attempt: generation_attempt,
                refresh_candidate,
            };
            loop {
                match poll_with_stack_boundary(self.backend.run_attempt(owner, context)).await {
                    StaAttemptOutcome::Advanced { owner: advanced } => owner = advanced,
                    StaAttemptOutcome::Stopped { owner } => {
                        return StaLifecycleExit::Stopped { owner, progress };
                    }
                    StaAttemptOutcome::Disconnected {
                        owner: returned,
                        next_candidate,
                    } => {
                        progress.connected_epochs = progress.connected_epochs.saturating_add(1);
                        progress.last_failure_stage = None;
                        generation_attempt = 1;
                        refresh_candidate = next_candidate == StaNextCandidate::Refresh;
                        owner = match poll_with_stack_boundary(self.backend.wait_backoff(
                            returned,
                            self.policy.disconnect_backoff_millis(),
                            StaBackoffReason::Disconnected,
                        ))
                        .await
                        {
                            StaBackoffOutcome::Elapsed { owner } => owner,
                            StaBackoffOutcome::Stopped { owner } => {
                                return StaLifecycleExit::Stopped { owner, progress };
                            }
                        };
                        break;
                    }
                    StaAttemptOutcome::Failed {
                        owner: returned,
                        failure,
                    } => {
                        progress.last_failure_stage = Some(failure.stage);
                        if failure.disposition == StaFailureDisposition::Terminal {
                            return StaLifecycleExit::Terminal {
                                owner: returned,
                                progress,
                                failure,
                            };
                        }
                        if generation_attempt >= self.policy.attempt_limit() {
                            return StaLifecycleExit::Exhausted {
                                owner: returned,
                                progress,
                                failure,
                            };
                        }
                        refresh_candidate =
                            failure.disposition == StaFailureDisposition::RefreshCandidate;
                        owner = match poll_with_stack_boundary(self.backend.wait_backoff(
                            returned,
                            self.policy.retry_backoff_millis(generation_attempt),
                            StaBackoffReason::AttemptFailed {
                                stage: failure.stage,
                                attempt: generation_attempt,
                            },
                        ))
                        .await
                        {
                            StaBackoffOutcome::Elapsed { owner } => owner,
                            StaBackoffOutcome::Stopped { owner } => {
                                return StaLifecycleExit::Stopped { owner, progress };
                            }
                        };
                        generation_attempt += 1;
                        break;
                    }
                    StaAttemptOutcome::Faulted { fault } => {
                        let reason = fault.reason();
                        return StaLifecycleExit::Faulted {
                            fault,
                            reason,
                            progress,
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
