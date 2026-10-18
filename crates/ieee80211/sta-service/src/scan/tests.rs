use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll},
};
use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Error {
    Begin,
    Channel(u8),
    Selection,
}

struct Backend {
    events: Vec<&'static str>,
    contexts: Vec<StaScanChannelContext<u8>>,
    begin: Result<(), Error>,
    fail_channel: Option<u8>,
    selection: Result<Option<u8>, Error>,
    stop_at_begin: bool,
}

impl StaCandidateScanBackend for Backend {
    type Owner = u32;
    type Channel = u8;
    type Candidate = u8;
    type Error = Error;

    async fn begin_scan(
        &mut self,
        owner: Self::Owner,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
        self.events.push("begin");
        let owner = owner + 1;
        if self.stop_at_begin {
            StaScanStepOutcome::Stopped { owner }
        } else {
            match self.begin {
                Ok(()) => StaScanStepOutcome::Completed { owner },
                Err(error) => StaScanStepOutcome::Failed { owner, error },
            }
        }
    }

    async fn scan_channel(
        &mut self,
        owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
        self.events.push("channel");
        self.contexts.push(context);
        let owner = owner + 1;
        if self.fail_channel == Some(context.channel) {
            StaScanStepOutcome::Failed {
                owner,
                error: Error::Channel(context.channel),
            }
        } else {
            StaScanStepOutcome::Completed { owner }
        }
    }

    fn select_candidate(
        &mut self,
        owner: Self::Owner,
    ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error> {
        self.events.push("select");
        let owner = owner + 1;
        match self.selection {
            Ok(Some(candidate)) => StaScanSelectionOutcome::Selected { owner, candidate },
            Ok(None) => StaScanSelectionOutcome::NoCandidate { owner },
            Err(error) => StaScanSelectionOutcome::Failed { owner, error },
        }
    }
}

fn backend(selection: Result<Option<u8>, Error>) -> Backend {
    Backend {
        events: Vec::new(),
        contexts: Vec::new(),
        begin: Ok(()),
        fail_channel: None,
        selection,
        stop_at_begin: false,
    }
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
fn selected_candidate_follows_the_complete_ordered_channel_plan() {
    let backend = backend(Ok(Some(42)));
    let mut service = StaCandidateScanService::new(backend);

    let exit = block_on(service.run(10, &[1, 6, 11]));

    assert_eq!(
        exit,
        StaCandidateScanExit::Selected {
            owner: 15,
            candidate: 42,
            progress: StaScanProgress {
                channels_planned: 3,
                channels_started: 3,
                channels_completed: 3,
            },
        }
    );
    assert_eq!(
        service.backend().contexts,
        [
            StaScanChannelContext {
                channel: 1,
                index: 0,
                total_channels: 3,
            },
            StaScanChannelContext {
                channel: 6,
                index: 1,
                total_channels: 3,
            },
            StaScanChannelContext {
                channel: 11,
                index: 2,
                total_channels: 3,
            },
        ]
    );
    assert!(service.backend().contexts[2].is_last());
}

#[test]
fn empty_plan_fails_before_the_backend_and_preserves_the_owner() {
    let backend = backend(Ok(Some(1)));
    let mut service = StaCandidateScanService::new(backend);

    assert_eq!(
        block_on(service.run(7, &[])),
        StaCandidateScanExit::InvalidPlan {
            owner: 7,
            error: StaScanPlanError::Empty,
            progress: StaScanProgress::default(),
        }
    );
    assert!(service.backend().events.is_empty());
}

#[test]
fn channel_failure_returns_the_exact_owner_and_stops_the_plan() {
    let mut backend = backend(Ok(Some(1)));
    backend.fail_channel = Some(6);
    let mut service = StaCandidateScanService::new(backend);

    assert_eq!(
        block_on(service.run(20, &[1, 6, 11])),
        StaCandidateScanExit::Failed {
            owner: 23,
            error: Error::Channel(6),
            progress: StaScanProgress {
                channels_planned: 3,
                channels_started: 2,
                channels_completed: 1,
            },
        }
    );
    assert_eq!(service.backend().contexts.len(), 2);
    assert_eq!(service.backend().events, ["begin", "channel", "channel"]);
}

#[test]
fn no_candidate_is_distinct_from_hardware_or_selection_failure() {
    let mut absent = StaCandidateScanService::new(backend(Ok(None)));
    assert!(matches!(
        block_on(absent.run(0, &[1])),
        StaCandidateScanExit::NoCandidate {
            owner: 3,
            progress: StaScanProgress {
                channels_completed: 1,
                ..
            },
        }
    ));

    let mut failed = StaCandidateScanService::new(backend(Err(Error::Selection)));
    assert!(matches!(
        block_on(failed.run(0, &[1])),
        StaCandidateScanExit::Failed {
            owner: 3,
            error: Error::Selection,
            ..
        }
    ));
}

#[test]
fn stop_during_preparation_returns_before_any_channel_is_started() {
    let mut backend = backend(Ok(Some(1)));
    backend.stop_at_begin = true;
    let mut service = StaCandidateScanService::new(backend);

    assert!(matches!(
        block_on(service.run(90, &[1, 6])),
        StaCandidateScanExit::Stopped {
            owner: 91,
            progress: StaScanProgress {
                channels_planned: 2,
                channels_started: 0,
                channels_completed: 0,
            },
        }
    ));
    assert_eq!(service.backend().events, ["begin"]);
}

#[test]
fn begin_failure_is_not_reclassified_as_no_candidate() {
    let mut backend = backend(Ok(Some(1)));
    backend.begin = Err(Error::Begin);
    let mut service = StaCandidateScanService::new(backend);

    assert!(matches!(
        block_on(service.run(4, &[1])),
        StaCandidateScanExit::Failed {
            owner: 5,
            error: Error::Begin,
            ..
        }
    ));
}
