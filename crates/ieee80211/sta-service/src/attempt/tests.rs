use core::future::{Future, ready};

use oer_ieee80211_sta::{attempt::StaConnectedEntryFailure, station::StaFailureDisposition};

use crate::test_support::block_on;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Owner {
    identity: u32,
    completed: u8,
    calls: [Option<StaAttemptStage>; StaAttemptStage::COUNT as usize],
    call_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Connected {
    identity: u32,
    calls: [Option<StaAttemptStage>; StaAttemptStage::COUNT as usize],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Error(StaAttemptStage);

#[derive(Clone, Copy)]
struct MockPort {
    fail_at: Option<StaAttemptStage>,
}

impl MockPort {
    fn new(fail_at: Option<StaAttemptStage>) -> Self {
        Self { fail_at }
    }

    fn step(
        &mut self,
        owner: &mut Owner,
        stage: StaAttemptStage,
    ) -> Result<(), StaAttemptStepError<Error>> {
        owner.calls[owner.call_count] = Some(stage);
        owner.call_count += 1;
        if self.fail_at == Some(stage) {
            return Err(StaAttemptStepError::retry_current(Error(stage)));
        }
        owner.completed += 1;
        Ok(())
    }
}

impl StaAttemptPort for MockPort {
    type Owner = Owner;
    type Connected = Connected;
    type Error = Error;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::Candidate))
    }

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::Channel))
    }

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::Authentication))
    }

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::Association))
    }

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::PeerProgramming))
    }

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::Wpa2Handshake))
    }

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a {
        ready(self.step(owner, StaAttemptStage::RsnKeyInstall))
    }

    fn enter_connected(
        &mut self,
        mut owner: Self::Owner,
    ) -> impl Future<
        Output = Result<Self::Connected, StaConnectedEntryFailure<Self::Owner, Self::Error>>,
    > + '_ {
        owner.calls[owner.call_count] = Some(StaAttemptStage::ConnectedEntry);
        owner.call_count += 1;
        if self.fail_at == Some(StaAttemptStage::ConnectedEntry) {
            return ready(Err(StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::Terminal,
                Error(StaAttemptStage::ConnectedEntry),
            )));
        }
        owner.completed += 1;
        ready(Ok(Connected {
            identity: owner.identity,
            calls: owner.calls,
        }))
    }
}

#[derive(Default)]
struct Observer {
    started: [Option<StaAttemptStage>; StaAttemptStage::COUNT as usize],
    completed: [Option<StaAttemptStage>; StaAttemptStage::COUNT as usize],
    failed: Option<(StaAttemptStage, StaFailureDisposition)>,
    started_count: usize,
    completed_count: usize,
}

impl StaAttemptObserver for Observer {
    fn stage_started(&mut self, stage: StaAttemptStage) {
        self.started[self.started_count] = Some(stage);
        self.started_count += 1;
    }

    fn stage_completed(&mut self, stage: StaAttemptStage) {
        self.completed[self.completed_count] = Some(stage);
        self.completed_count += 1;
    }

    fn stage_failed(&mut self, stage: StaAttemptStage, disposition: StaFailureDisposition) {
        self.failed = Some((stage, disposition));
    }
}

const STAGES: [StaAttemptStage; StaAttemptStage::COUNT as usize] = [
    StaAttemptStage::Candidate,
    StaAttemptStage::Channel,
    StaAttemptStage::Authentication,
    StaAttemptStage::Association,
    StaAttemptStage::PeerProgramming,
    StaAttemptStage::Wpa2Handshake,
    StaAttemptStage::RsnKeyInstall,
    StaAttemptStage::ConnectedEntry,
];

#[test]
fn complete_attempt_orders_every_phase_once() {
    let mut attempt = StaAttempt::with_observer(MockPort::new(None), Observer::default());
    let outcome = block_on(attempt.run(Owner {
        identity: 41,
        completed: 0,
        calls: [None; StaAttemptStage::COUNT as usize],
        call_count: 0,
    }));
    let AssociationAttemptOutcome::Connected {
        connected,
        progress,
    } = outcome
    else {
        panic!("complete attempt failed");
    };
    assert_eq!(
        connected,
        Connected {
            identity: 41,
            calls: STAGES.map(Some),
        }
    );
    assert_eq!(progress.completed_count(), StaAttemptStage::COUNT);
    for stage in STAGES {
        assert!(progress.completed(stage));
    }
    let (_, observer) = attempt.into_parts();
    assert_eq!(observer.started, STAGES.map(Some));
    assert_eq!(observer.completed, STAGES.map(Some));
    assert_eq!(observer.failed, None);
}

#[test]
fn every_borrowed_phase_failure_returns_the_exact_owner_and_frontier() {
    for (failed_index, failed_stage) in STAGES[..STAGES.len() - 1].iter().enumerate() {
        let mut attempt = StaAttempt::new(MockPort::new(Some(*failed_stage)));
        let outcome = block_on(attempt.run(Owner {
            identity: 77,
            completed: 0,
            calls: [None; StaAttemptStage::COUNT as usize],
            call_count: 0,
        }));
        let AssociationAttemptOutcome::Failed(failure) = outcome else {
            panic!("failed phase reached connected frontier");
        };
        assert_eq!(failure.owner.identity, 77);
        assert_eq!(failure.owner.completed, failed_index as u8);
        assert_eq!(failure.stage, *failed_stage);
        assert_eq!(
            failure.disposition,
            StaFailureDisposition::RetryCurrentCandidate
        );
        assert_eq!(failure.error, Error(*failed_stage));
        assert_eq!(failure.progress.completed_count(), failed_index as u8);
        for (call, expected) in failure.owner.calls[..=failed_index]
            .iter()
            .zip(&STAGES[..=failed_index])
        {
            assert_eq!(*call, Some(*expected));
        }
    }
}

#[test]
fn connected_entry_failure_must_return_the_consumed_owner() {
    let mut attempt = StaAttempt::new(MockPort::new(Some(StaAttemptStage::ConnectedEntry)));
    let outcome = block_on(attempt.run(Owner {
        identity: 91,
        completed: 0,
        calls: [None; StaAttemptStage::COUNT as usize],
        call_count: 0,
    }));
    let AssociationAttemptOutcome::Failed(failure) = outcome else {
        panic!("connected entry unexpectedly passed");
    };
    assert_eq!(failure.owner.identity, 91);
    assert_eq!(failure.owner.completed, 7);
    assert_eq!(failure.stage, StaAttemptStage::ConnectedEntry);
    assert_eq!(failure.disposition, StaFailureDisposition::Terminal);
    assert_eq!(failure.progress.completed_count(), 7);
    assert!(!failure.progress.completed(StaAttemptStage::ConnectedEntry));
}
