//! The access point's retire step over a runner of the test and the port of
//! the station's news tests, as the chip's runner calls it.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

extern crate std;

use super::*;
use crate::failure::{BackendFault, CleanupCause, ReleaseCause};
use crate::release::Unreleased;
use crate::station_end::tests::{Leaves, Radio, Receipt, TestPort, leave, ready};
use crate::unfinished::Phase;
use oer_portable_time_virtual::VirtualClock;

/// An ended runner of the test: its port, another owner its port holds,
/// how many times its stop was watched, and the items its port calls could
/// not keep. Its stop needs a service, which it makes none of while it
/// holds an item: the watch waits for good then, as the 6639a1170 runner
/// did.
struct TestRunner<'a, 'b> {
    port: TestPort<'a>,
    /// Another owner of the port, which ends with the runner.
    other: Option<Receipt<'a>>,
    watched: &'b Cell<u8>,
    /// The items the port's calls could not keep, the next one first.
    unkept: std::vec::Vec<Receipt<'a>>,
    /// The items the watches' services return, one a watch, before the
    /// stop is confirmed.
    found: std::vec::Vec<Receipt<'a>>,
    confirmed: &'b Cell<bool>,
    /// An attempt a submission returned unaccepted.
    unaccepted: Option<Receipt<'a>>,
    /// The port keeps a fault it could not take back: a watch without an
    /// item ends on this observation.
    kept_fault: Option<StopUnconfirmed>,
    /// That observation is the stop's own answer naming a device fault, no
    /// port call having answered one: the watch ends on it named.
    device_first: bool,
}

impl<'a> Retiring for TestRunner<'a, '_> {
    type Port = TestPort<'a>;
    type Unkept = Receipt<'a>;
    fn finish_stop(
        &mut self,
        _unconfirmed: impl FnMut(StopUnconfirmed),
    ) -> impl Future<Output = Result<StopWatch<BackendFault>, ReceiveAccounting>> {
        self.watched.set(self.watched.get() + 1);
        let holds = !self.unkept.is_empty();
        // The watch's service returns the next item it could not keep.
        let found = if holds { None } else { self.found.pop() };
        let watch = match (found, self.kept_fault) {
            (Some(item), _) => {
                self.unkept.push(item);
                Some(StopWatch::Unkept)
            }
            (None, _) if holds => None,
            (None, Some(observed)) if self.device_first => Some(StopWatch::DeviceFailed {
                fault: None,
                observed,
            }),
            (None, Some(observed)) => Some(StopWatch::Faulted {
                fault: Some(BackendFault::Abandoned),
                observed,
            }),
            (None, None) => Some(StopWatch::Confirmed),
        };
        let confirmed = self.confirmed;
        core::future::poll_fn(move |_| match watch {
            // No service while it holds an item: no confirmation either.
            None => core::task::Poll::Pending,
            Some(watch) => {
                if watch == StopWatch::Confirmed {
                    confirmed.set(true);
                }
                core::task::Poll::Ready(Ok(watch))
            }
        })
    }
    fn port_mut(&mut self) -> &mut TestPort<'a> {
        &mut self.port
    }
    fn take_unkept(&mut self) -> Option<Receipt<'a>> {
        self.unkept.pop()
    }
    type Unaccepted = Receipt<'a>;
    fn take_unaccepted(&mut self) -> Option<Receipt<'a>> {
        self.unaccepted.take()
    }
}

type Exit = ApExitOf<()>;

/// What a retire of the test left: its exit, its unfinished whole if one,
/// the last publication and how many times the stop was watched.
struct Left<'a, 'b> {
    exit: Exit,
    unfinished: Option<
        UnfinishedAccessPoint<TestRunner<'a, 'b>, Receipt<'a>, Receipt<'a>, Receipt<'a>, ()>,
    >,
    published: Option<Exit>,
    watched: u8,
}

fn retired_with<'a, 'b>(runner: TestRunner<'a, 'b>, ended: ApEnded, stop: Stop) -> Left<'a, 'b> {
    let clock = VirtualClock::<2>::new();
    let published = Cell::new(None);
    let watched = runner.watched;
    let retired = ready(retire(
        &published,
        runner,
        ended,
        stop,
        &clock,
        Duration::from_millis(1),
        leave,
    ));
    let (exit, unfinished) = match retired {
        Retired::Completed(exit) => (exit, None),
        Retired::Unfinished(unfinished) => (unfinished.cause, Some(unfinished)),
    };
    Left {
        exit,
        unfinished,
        published: published.get(),
        watched: watched.get(),
    }
}

/// Retire a runner over `port` that holds `unkept`, as the run ended.
fn retired<'a, 'b>(
    port: TestPort<'a>,
    unkept: Option<Receipt<'a>>,
    stop: Stop,
    watched: &'b Cell<u8>,
    confirmed: &'b Cell<bool>,
) -> Left<'a, 'b> {
    let runner = TestRunner {
        port,
        other: None,
        watched,
        unkept: unkept.into_iter().collect(),
        found: std::vec::Vec::new(),
        confirmed,
        unaccepted: None,
        kept_fault: None,
        device_first: false,
    };
    retired_with(runner, ApEnded::Stopped, stop)
}

#[test]
fn a_refused_release_returns_its_receipt_from_the_retire_to_the_runner_caller() {
    let refusals = [
        (Ok(()), Err(Unreleased::Held), ReleaseCause::Held),
        (
            Ok(()),
            Ok(Radio(Leaves::Ended(ReleaseCause::LeaveUnanswered([
                4, 3, 2, 1,
            ])))),
            ReleaseCause::LeaveUnanswered([4, 3, 2, 1]),
        ),
        (
            Err(CleanupCause::Faulted(BackendFault::Abandoned)),
            Ok(Radio(Leaves::Left)),
            ReleaseCause::Cleanup(CleanupCause::Faulted(BackendFault::Abandoned)),
        ),
    ];
    for (cleanup, stopped, cause) in refusals {
        let ends = Cell::new(0);
        let port = TestPort {
            cleanup,
            receipt: Some(Receipt(&ends)),
            stopped,
        };
        let (watched, confirmed) = (Cell::new(0), Cell::new(false));
        let left = retired(port, None, Stop::Confirmed, &watched, &confirmed);
        assert_eq!(
            left.exit,
            ApExitOf::ReleaseRefused {
                ended: ApEnded::Stopped,
                cause
            }
        );
        assert_eq!(left.watched, 0, "a confirmed stop is not watched again");
        assert_eq!(ends.get(), 0, "nothing ended");
        let Some(unfinished) = left.unfinished else {
            panic!("a refused release leaves the installation unfinished");
        };
        assert_eq!(unfinished.phase, Phase::Releasing);
        assert!(unfinished.receipt.is_some(), "the caller holds the receipt");
        drop(unfinished);
        assert_eq!(ends.get(), 1);
    }
}

#[test]
fn a_release_that_succeeded_ends_its_receipt_in_the_retire() {
    let ends = Cell::new(0);
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&ends)),
        stopped: Ok(Radio(Leaves::Left)),
    };
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let left = retired(port, None, Stop::Owed, &watched, &confirmed);
    assert_eq!(left.exit, ApExitOf::Ended(ApEnded::Stopped));
    assert_eq!(left.watched, 1, "an owed stop is watched first");
    assert!(left.unfinished.is_none(), "a completed retire");
    assert_eq!(ends.get(), 1);
}

#[test]
fn an_unconfirmed_stop_is_published_then_watched_before_the_release() {
    let ends = Cell::new(0);
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&ends)),
        stopped: Err(Unreleased::Held),
    };
    let observed = StopUnconfirmed {
        cause: oer_protocols_ieee80211_contracts_port::StopCause::Deadline,
        observations: Default::default(),
    };
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let left = retired(
        port,
        None,
        Stop::Unconfirmed(observed),
        &watched,
        &confirmed,
    );
    assert_eq!(left.watched, 1);
    // The observation was published; a release refusal that is not
    // repeated is the exit, not a publication.
    assert_eq!(
        left.published,
        Some(ApExitOf::StopUnconfirmed {
            ended: ApEnded::Stopped,
            observed
        })
    );
    assert_eq!(
        left.exit,
        ApExitOf::ReleaseRefused {
            ended: ApEnded::Stopped,
            cause: ReleaseCause::Held
        }
    );
    assert!(
        left.unfinished
            .is_some_and(|unfinished| unfinished.receipt.is_some())
    );
}

#[test]
fn an_item_the_run_could_not_keep_skips_the_release_and_goes_back_to_the_caller() {
    let cleanups = Cell::new(0);
    let ends = Cell::new(0);
    // The port's receipt would be freed by a cleanup, which does not run.
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&cleanups)),
        stopped: Ok(Radio(Leaves::Left)),
    };
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let left = retired(
        port,
        Some(Receipt(&ends)),
        Stop::Confirmed,
        &watched,
        &confirmed,
    );
    assert_eq!(
        left.exit,
        ApExitOf::Unkept {
            ended: ApEnded::Stopped,
            stop: UnkeptStop::Confirmed
        }
    );
    let Some(mut unfinished) = left.unfinished else {
        panic!("the item leaves the installation unfinished");
    };
    assert_eq!(unfinished.phase, Phase::Stopped);
    assert_eq!(ends.get(), 0, "the item is not ended");
    assert!(unfinished.unkept[0].is_some(), "the caller holds the item");
    // The port, with the receipt no cleanup freed, came back with it.
    assert!(unfinished.owner.port.receipt.take().is_some());
    drop(unfinished);
    assert_eq!((ends.get(), cleanups.get()), (1, 1));
}

/// The owner's counterexample of 6639a1170: the retire handed the item
/// back and dropped the runner, and with it every other owner of the port
/// it never released. Every owner leaves together now.
#[test]
fn an_unkept_item_leaves_with_every_other_owner_of_the_unreleased_port() {
    let item = Cell::new(0);
    let other = Cell::new(0);
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: None,
            stopped: Ok(Radio(Leaves::Left)),
        },
        other: Some(Receipt(&other)),
        watched: &watched,
        unkept: std::vec![Receipt(&item)],
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: None,
        kept_fault: None,
        device_first: false,
    };
    let left = retired_with(runner, ApEnded::Unkept, Stop::Confirmed);
    assert_eq!(item.get(), 0, "the item is alive");
    assert_eq!(
        other.get(),
        0,
        "the unreleased port's other owner is alive beside the item"
    );
    let Some(unfinished) = left.unfinished else {
        panic!("the installation is unfinished");
    };
    assert!(
        unfinished.owner.other.is_some(),
        "the runner came back whole"
    );
    drop(unfinished);
    assert_eq!((item.get(), other.get()), (1, 1));
}

/// The owner's counterexample of 6639a1170: the retire waited for the
/// stop, which needs the service the runner makes none of while it holds
/// an item, before it took the item: a cyclic wait. The item goes to the
/// retire's home first, and the stop is watched to its confirmation.
#[test]
fn a_stop_that_needs_a_service_goes_on_once_the_unkept_item_is_taken() {
    let item = Cell::new(0);
    let other = Cell::new(0);
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: None,
            stopped: Ok(Radio(Leaves::Left)),
        },
        other: Some(Receipt(&other)),
        watched: &watched,
        unkept: std::vec![Receipt(&item)],
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: None,
        kept_fault: None,
        device_first: false,
    };
    let left = retired_with(runner, ApEnded::Unkept, Stop::Owed);
    assert!(confirmed.get(), "the stop was watched to its confirmation");
    assert_eq!(
        left.exit,
        ApExitOf::Unkept {
            ended: ApEnded::Unkept,
            stop: UnkeptStop::Confirmed
        }
    );
    assert_eq!(item.get(), 0);
    let Some(unfinished) = left.unfinished else {
        panic!("the item leaves the installation unfinished");
    };
    assert_eq!(unfinished.phase, Phase::Stopped);
    assert!(unfinished.unkept[0].is_some());
    drop(unfinished);
    assert_eq!((item.get(), other.get()), (1, 1));
}

/// Each watch that returned an item goes on once the retire took it, while
/// its home has a free place: the next item stays with the runner, which
/// comes back with every item in the stop's phase.
#[test]
fn items_the_stop_finds_fill_the_home_then_the_next_stays_with_the_runner() {
    let ends = Cell::new(0);
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: None,
            stopped: Ok(Radio(Leaves::Left)),
        },
        other: None,
        watched: &watched,
        unkept: std::vec::Vec::new(),
        found: (0..=UNKEPT_ITEMS).map(|_| Receipt(&ends)).collect(),
        confirmed: &confirmed,
        unaccepted: None,
        kept_fault: None,
        device_first: false,
    };
    let left = retired_with(runner, ApEnded::Stopped, Stop::Owed);
    assert!(!confirmed.get());
    // The stop is not confirmed, and the exit says so.
    assert_eq!(
        left.exit,
        ApExitOf::Unkept {
            ended: ApEnded::Stopped,
            stop: UnkeptStop::Unconfirmed
        }
    );
    let Some(unfinished) = left.unfinished else {
        panic!("a full home ends the retire unfinished");
    };
    assert_eq!(unfinished.phase, Phase::Stopping);
    assert!(unfinished.unkept.iter().all(Option::is_some));
    assert_eq!(
        unfinished.owner.unkept.len(),
        1,
        "the runner keeps the next"
    );
    assert_eq!(ends.get(), 0);
    drop(unfinished);
    assert_eq!(ends.get(), UNKEPT_ITEMS as u8 + 1);
}

/// A run that ended on an attempt the port did not accept: the retire runs
/// no release, and the attempt comes back with the runner and every other
/// owner of the unreleased port, the stop confirmed.
#[test]
fn an_unaccepted_attempt_comes_back_with_the_unreleased_runner() {
    let attempt = Cell::new(0);
    let other = Cell::new(0);
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: None,
            stopped: Ok(Radio(Leaves::Left)),
        },
        other: Some(Receipt(&other)),
        watched: &watched,
        unkept: std::vec::Vec::new(),
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: Some(Receipt(&attempt)),
        kept_fault: None,
        device_first: false,
    };
    let left = retired_with(runner, ApEnded::Unaccepted, Stop::Owed);
    assert_eq!(
        left.exit,
        ApExitOf::Unaccepted {
            ended: ApEnded::Unaccepted,
            stop: UnkeptStop::Confirmed,
        }
    );
    assert_eq!((attempt.get(), other.get()), (0, 0), "nothing ended");
    let Some(unfinished) = left.unfinished else {
        panic!("no release runs while the attempt is out");
    };
    assert_eq!(unfinished.phase, Phase::Stopped);
    assert!(unfinished.unaccepted.is_some());
    assert!(unfinished.owner.port.stopped.is_ok(), "no release ran");
    drop(unfinished);
    assert_eq!((attempt.get(), other.get()), (1, 1), "each ended once");
}

/// The owner's decision 1: a port that keeps a fault it could not take
/// back ends the retire unfinished in the stopping phase, with the fault,
/// the port's last observation and every owner (an item it could not keep,
/// an attempt it did not accept, the runner with its port); no release
/// runs. A stop confirmed before the retire is not watched again, so a
/// kept fault does not downgrade it.
#[test]
fn a_kept_fault_ends_the_retire_unfinished_in_the_stopping_phase_with_every_owner() {
    let observed = StopUnconfirmed {
        cause: oer_protocols_ieee80211_contracts_port::StopCause::Device,
        observations: Default::default(),
    };
    let (ends, item, attempt) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: Some(Receipt(&ends)),
            stopped: Err(Unreleased::StopUnconfirmed),
        },
        other: Some(Receipt(&ends)),
        watched: &watched,
        unkept: std::vec![Receipt(&item)],
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: Some(Receipt(&attempt)),
        kept_fault: Some(observed),
        device_first: false,
    };
    let ended = ApEnded::Faulted(BackendFault::Abandoned);
    let left = retired_with(runner, ended, Stop::Owed);
    assert_eq!(
        left.exit,
        ApExitOf::StopFaulted {
            ended,
            fault: Some(BackendFault::Abandoned),
            observed,
        }
    );
    let Some(mut unfinished) = left.unfinished else {
        panic!("a kept fault leaves the installation unfinished");
    };
    assert_eq!(unfinished.phase, Phase::Stopping);
    assert!(!confirmed.get(), "no stop was confirmed");
    assert_eq!([ends.get(), item.get(), attempt.get()], [0, 0, 0]);
    assert!(unfinished.unkept[0].take().is_some(), "the item is out");
    assert!(unfinished.unaccepted.take().is_some(), "and the attempt");
    assert!(unfinished.receipt.is_none(), "no cleanup ran");
    assert!(unfinished.owner.port.receipt.is_some() && unfinished.owner.other.is_some());
    assert_eq!([item.get(), attempt.get()], [1, 1]);
    drop(unfinished);
    assert_eq!(ends.get(), 2, "the runner's owners end with their holder");

    // A stop the port confirmed before the retire is not watched again.
    let (ends, watched, confirmed) = (Cell::new(0), Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: Some(Receipt(&ends)),
            stopped: Ok(Radio(Leaves::Left)),
        },
        other: None,
        watched: &watched,
        unkept: std::vec::Vec::new(),
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: None,
        kept_fault: Some(observed),
        device_first: false,
    };
    let left = retired_with(runner, ApEnded::Stopped, Stop::Confirmed);
    assert_eq!(left.watched, 0);
    assert_eq!(left.exit, ApExitOf::Ended(ApEnded::Stopped));
    assert!(left.unfinished.is_none(), "a completed retire");
}

/// A device fault the stop's own answer named first ends the retire
/// unfinished in the stopping phase, named apart from a kept fault, with
/// the port's observation and every owner; no release runs.
#[test]
fn a_device_fault_the_stop_named_first_ends_the_retire_unfinished_and_named() {
    let observed = StopUnconfirmed {
        cause: oer_protocols_ieee80211_contracts_port::StopCause::Device,
        observations: Default::default(),
    };
    let (ends, attempt) = (Cell::new(0), Cell::new(0));
    let (watched, confirmed) = (Cell::new(0), Cell::new(false));
    let runner = TestRunner {
        port: TestPort {
            cleanup: Ok(()),
            receipt: Some(Receipt(&ends)),
            stopped: Err(Unreleased::StopUnconfirmed),
        },
        other: Some(Receipt(&ends)),
        watched: &watched,
        unkept: std::vec::Vec::new(),
        found: std::vec::Vec::new(),
        confirmed: &confirmed,
        unaccepted: Some(Receipt(&attempt)),
        kept_fault: Some(observed),
        device_first: true,
    };
    let ended = ApEnded::Faulted(BackendFault::Abandoned);
    let left = retired_with(runner, ended, Stop::Owed);
    assert_eq!(
        left.exit,
        ApExitOf::StopDeviceFailed {
            ended,
            fault: None,
            observed,
        }
    );
    let Some(mut unfinished) = left.unfinished else {
        panic!("a device fault leaves the installation unfinished");
    };
    assert_eq!(unfinished.phase, Phase::Stopping);
    assert!(!confirmed.get(), "no stop was confirmed");
    assert_eq!([ends.get(), attempt.get()], [0, 0]);
    assert!(unfinished.unaccepted.take().is_some(), "the attempt is out");
    assert!(unfinished.receipt.is_none(), "no cleanup ran");
    assert!(unfinished.owner.port.receipt.is_some() && unfinished.owner.other.is_some());
}
