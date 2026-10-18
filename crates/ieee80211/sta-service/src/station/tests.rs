use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll},
};
use std::{collections::VecDeque, vec::Vec};

use oer_ieee80211_sta::station::{
    StaAttemptFailure, StaFaultCause, StaFaultPhase, StaFaultReason, StaLifecycleStage,
};

use super::*;

#[derive(Clone, Copy)]
enum Planned {
    Advanced,
    Disconnected(StaNextCandidate),
    Stopped,
    Failed(StaLifecycleStage, StaFailureDisposition, u8),
    Faulted(Fault),
}

/// A planned fault frontier naming its portable reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fault {
    frontier: u64,
    reason: StaFaultReason,
}

impl StaFault for Fault {
    fn reason(&self) -> StaFaultReason {
        self.reason
    }
}

struct Backend {
    outcomes: VecDeque<Planned>,
    contexts: Vec<StaAttemptContext>,
    backoffs: Vec<(u32, StaBackoffReason)>,
    stop_in_backoff: bool,
}

impl StaLifecycleBackend for Backend {
    type Owner = u32;
    type Error = u8;
    type Fault = Fault;

    async fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault> {
        self.contexts.push(context);
        let owner = owner + 1;
        match self.outcomes.pop_front().expect("planned attempt") {
            Planned::Advanced => StaAttemptOutcome::Advanced { owner },
            Planned::Disconnected(next_candidate) => StaAttemptOutcome::Disconnected {
                owner,
                next_candidate,
            },
            Planned::Stopped => StaAttemptOutcome::Stopped { owner },
            Planned::Failed(stage, disposition, error) => StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(stage, disposition, error),
            },
            Planned::Faulted(fault) => StaAttemptOutcome::Faulted { fault },
        }
    }

    async fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        reason: StaBackoffReason,
    ) -> StaBackoffOutcome<Self::Owner> {
        self.backoffs.push((delay_millis, reason));
        if self.stop_in_backoff {
            StaBackoffOutcome::Stopped { owner }
        } else {
            StaBackoffOutcome::Elapsed { owner }
        }
    }
}

fn backend(outcomes: impl IntoIterator<Item = Planned>) -> Backend {
    Backend {
        outcomes: outcomes.into_iter().collect(),
        contexts: Vec::new(),
        backoffs: Vec::new(),
        stop_in_backoff: false,
    }
}

fn policy(attempt_limit: u16) -> StaReconnectPolicy {
    StaReconnectPolicy::new(attempt_limit, 10, 40, 5).unwrap()
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn advanced_phases_share_one_attempt_without_backoff() {
    let backend = backend([Planned::Advanced, Planned::Advanced, Planned::Stopped]);
    let mut service = StaLifecycleService::new(backend, policy(3));
    let exit = block_on(service.run(0));
    let StaLifecycleExit::Stopped { owner, progress } = exit else {
        panic!("advanced test must stop cleanly")
    };
    assert_eq!(owner, 3);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(progress.final_generation_attempt, 1);
    assert_eq!(service.backend().contexts.len(), 3);
    assert!(
        service
            .backend()
            .contexts
            .iter()
            .all(|context| *context == service.backend().contexts[0])
    );
    assert!(service.backend().backoffs.is_empty());
}

#[test]
fn retry_and_refresh_policy_preserve_one_owner_until_stop() {
    let backend = backend([
        Planned::Failed(
            StaLifecycleStage::Association,
            StaFailureDisposition::RetryCurrentCandidate,
            1,
        ),
        Planned::Failed(
            StaLifecycleStage::Security,
            StaFailureDisposition::RefreshCandidate,
            2,
        ),
        Planned::Stopped,
    ]);
    let mut service = StaLifecycleService::new(backend, policy(4));

    let exit = block_on(service.run(10));

    assert_eq!(
        exit,
        StaLifecycleExit::Stopped {
            owner: 13,
            progress: StaLifecycleProgress {
                connected_epochs: 0,
                attempts_started: 3,
                final_generation_attempt: 3,
                last_failure_stage: Some(StaLifecycleStage::Security),
            },
        }
    );
    assert_eq!(
        service.backend().contexts,
        [
            StaAttemptContext {
                generation: 0,
                attempt: 1,
                refresh_candidate: true,
            },
            StaAttemptContext {
                generation: 0,
                attempt: 2,
                refresh_candidate: false,
            },
            StaAttemptContext {
                generation: 0,
                attempt: 3,
                refresh_candidate: true,
            },
        ]
    );
    assert_eq!(service.backend().backoffs[0].0, 10);
    assert_eq!(service.backend().backoffs[1].0, 20);
}

#[test]
fn disconnect_starts_a_new_generation_after_nonzero_backoff() {
    let backend = backend([
        Planned::Disconnected(StaNextCandidate::Refresh),
        Planned::Stopped,
    ]);
    let mut service = StaLifecycleService::new(backend, policy(3));

    let exit = block_on(service.run(0));

    assert!(matches!(
        exit,
        StaLifecycleExit::Stopped {
            owner: 2,
            progress: StaLifecycleProgress {
                connected_epochs: 1,
                attempts_started: 2,
                final_generation_attempt: 1,
                last_failure_stage: None,
            },
        }
    ));
    assert_eq!(
        service.backend().backoffs,
        [(5, StaBackoffReason::Disconnected)]
    );
    assert_eq!(service.backend().contexts[1].generation, 1);
    assert!(service.backend().contexts[1].refresh_candidate);
}

#[test]
fn same_peer_reconnect_does_not_claim_an_unperformed_candidate_refresh() {
    let backend = backend([
        Planned::Disconnected(StaNextCandidate::Reuse),
        Planned::Stopped,
    ]);
    let mut service = StaLifecycleService::new(backend, policy(3));

    let exit = block_on(service.run(7));

    assert!(matches!(exit, StaLifecycleExit::Stopped { owner: 9, .. }));
    assert_eq!(
        service.backend().contexts,
        [
            StaAttemptContext {
                generation: 0,
                attempt: 1,
                refresh_candidate: true,
            },
            StaAttemptContext {
                generation: 1,
                attempt: 1,
                refresh_candidate: false,
            },
        ]
    );
}

#[test]
fn exhaustion_returns_the_last_failure_and_exact_owner_without_extra_wait() {
    let backend = backend([
        Planned::Failed(
            StaLifecycleStage::Authentication,
            StaFailureDisposition::RetryCurrentCandidate,
            7,
        ),
        Planned::Failed(
            StaLifecycleStage::Authentication,
            StaFailureDisposition::RetryCurrentCandidate,
            8,
        ),
    ]);
    let mut service = StaLifecycleService::new(backend, policy(2));

    let exit = block_on(service.run(4));

    assert!(matches!(
        exit,
        StaLifecycleExit::Exhausted {
            owner: 6,
            failure: StaAttemptFailure { error: 8, .. },
            ..
        }
    ));
    assert_eq!(service.backend().backoffs.len(), 1);
}

#[test]
fn terminal_failure_and_backoff_stop_both_return_hardware_ownership() {
    let terminal = backend([Planned::Failed(
        StaLifecycleStage::Hardware,
        StaFailureDisposition::Terminal,
        9,
    )]);
    let mut service = StaLifecycleService::new(terminal, policy(3));
    assert!(matches!(
        block_on(service.run(20)),
        StaLifecycleExit::Terminal {
            owner: 21,
            failure: StaAttemptFailure { error: 9, .. },
            ..
        }
    ));

    let mut stopped = backend([Planned::Failed(
        StaLifecycleStage::Security,
        StaFailureDisposition::RefreshCandidate,
        3,
    )]);
    stopped.stop_in_backoff = true;
    let mut service = StaLifecycleService::new(stopped, policy(3));
    assert!(matches!(
        block_on(service.run(30)),
        StaLifecycleExit::Stopped { owner: 31, .. }
    ));
}

#[test]
fn faulted_frontier_is_returned_without_retry_or_backoff() {
    let fault = Fault {
        frontier: 0xfeed_beef,
        reason: StaFaultReason::new(StaFaultPhase::Teardown, StaFaultCause::Security),
    };
    let backend = backend([Planned::Faulted(fault)]);
    let mut service = StaLifecycleService::new(backend, policy(3));

    assert_eq!(
        block_on(service.run(40)),
        StaLifecycleExit::Faulted {
            fault,
            reason: StaFaultReason::new(StaFaultPhase::Teardown, StaFaultCause::Security),
            progress: StaLifecycleProgress {
                connected_epochs: 0,
                attempts_started: 1,
                final_generation_attempt: 1,
                last_failure_stage: None,
            },
        }
    );
    assert_eq!(fault.frontier, 0xfeed_beef);
    assert!(service.backend().backoffs.is_empty());
    assert_eq!(service.backend().contexts.len(), 1);
}
