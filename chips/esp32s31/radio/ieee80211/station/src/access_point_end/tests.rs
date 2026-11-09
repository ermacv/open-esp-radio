//! The access point's retire step over a runner of the test and the port of
//! the station's news tests, as the chip's runner calls it.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use super::*;
use crate::failure::{BackendFault, CleanupCause, ReleaseCause};
use crate::station_end::tests::{Leaves, Radio, Receipt, TestPort, leave, ready};
use oer_portable_time_virtual::VirtualClock;

/// An ended runner of the test: its port, and how many times its stop was
/// watched.
struct TestRunner<'a, 'b> {
    port: TestPort<'a>,
    watched: &'b Cell<u8>,
    /// The item a port call of the run could not keep.
    unkept: Option<Receipt<'a>>,
}

impl<'a> Retiring for TestRunner<'a, '_> {
    type Port = TestPort<'a>;
    type Unrecycled = ();
    async fn finish_stop(
        &mut self,
        _unconfirmed: impl FnMut(StopUnconfirmed),
    ) -> Result<(), ReceiveAccounting> {
        self.watched.set(self.watched.get() + 1);
        Ok(())
    }
    fn take_unrecycled(&mut self) {}
    fn port_mut(&mut self) -> &mut TestPort<'a> {
        &mut self.port
    }
    fn take_unkept(&mut self) -> Option<Receipt<'a>> {
        self.unkept.take()
    }
}

type Exit = ApExitOf<()>;

fn retired<'a>(
    port: TestPort<'a>,
    unkept: Option<Receipt<'a>>,
    stop: Stop,
) -> (Exit, Option<Receipt<'a>>, Option<Exit>, u8) {
    let clock = VirtualClock::<2>::new();
    let published = Cell::new(None);
    let watched = Cell::new(0);
    let runner = TestRunner {
        port,
        watched: &watched,
        unkept,
    };
    let (exit, returned) = ready(retire(
        &published,
        runner,
        ApEnded::Stopped,
        stop,
        &clock,
        Duration::from_millis(1),
        leave,
    ));
    (exit, returned, published.get(), watched.get())
}

#[test]
fn a_refused_release_returns_its_receipt_from_the_retire_to_the_runner_caller() {
    let refusals = [
        (Ok(()), None, ReleaseCause::NotStopped),
        (
            Ok(()),
            Some(Radio(Leaves::Ended(ReleaseCause::LeaveUnanswered([
                4, 3, 2, 1,
            ])))),
            ReleaseCause::LeaveUnanswered([4, 3, 2, 1]),
        ),
        (
            Err(CleanupCause::Faulted(BackendFault::Abandoned)),
            Some(Radio(Leaves::Left)),
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
        let (exit, returned, _, watched) = retired(port, None, Stop::Confirmed);
        assert_eq!(
            exit,
            ApExitOf::ReleaseRefused {
                ended: ApEnded::Stopped,
                cause
            }
        );
        assert_eq!(watched, 0, "a confirmed stop is not watched again");
        assert_eq!(ends.get(), 0, "the runner ended without the receipt");
        assert!(returned.is_some(), "the caller holds the receipt");
        drop(returned);
        assert_eq!(ends.get(), 1);
    }
}

#[test]
fn a_release_that_succeeded_ends_its_receipt_in_the_retire() {
    let ends = Cell::new(0);
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&ends)),
        stopped: Some(Radio(Leaves::Left)),
    };
    let (exit, returned, _, watched) = retired(port, None, Stop::Owed);
    assert_eq!(exit, ApExitOf::Ended(ApEnded::Stopped));
    assert_eq!(watched, 1, "an owed stop is watched first");
    assert!(returned.is_none());
    assert_eq!(ends.get(), 1);
}

#[test]
fn an_unconfirmed_stop_is_published_then_watched_before_the_release() {
    let ends = Cell::new(0);
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&ends)),
        stopped: None,
    };
    let observed = StopUnconfirmed {
        cause: oer_protocols_ieee80211_contracts_port::StopCause::Deadline,
        observations: Default::default(),
    };
    let (exit, returned, published, watched) = retired(port, None, Stop::Unconfirmed(observed));
    assert_eq!(watched, 1);
    // The observation was published; a release refusal that is not
    // repeated is the exit, not a publication.
    assert_eq!(
        published,
        Some(ApExitOf::StopUnconfirmed {
            ended: ApEnded::Stopped,
            observed
        })
    );
    assert_eq!(
        exit,
        ApExitOf::ReleaseRefused {
            ended: ApEnded::Stopped,
            cause: ReleaseCause::NotStopped
        }
    );
    assert!(returned.is_some());
}

#[test]
fn an_item_the_run_could_not_keep_skips_the_release_and_goes_back_to_the_caller() {
    let cleanups = Cell::new(0);
    let ends = Cell::new(0);
    // The port's receipt would be freed by a cleanup, which does not run.
    let port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&cleanups)),
        stopped: Some(Radio(Leaves::Left)),
    };
    let (exit, returned, _, _) = retired(port, Some(Receipt(&ends)), Stop::Confirmed);
    assert_eq!(
        exit,
        ApExitOf::ReleaseRefused {
            ended: ApEnded::Stopped,
            cause: ReleaseCause::Unkept
        }
    );
    assert_eq!(ends.get(), 0, "the item is not ended");
    assert!(returned.is_some(), "the caller holds the item");
    drop(returned);
    assert_eq!(ends.get(), 1);
}
