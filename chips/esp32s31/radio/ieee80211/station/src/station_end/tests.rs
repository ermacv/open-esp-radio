//! The station's news path over a port of the test: the actor side's
//! [`reinstalled`] and [`retired`], read by the station's [`installed`] and
//! [`ended`], as the chip's tasks call them.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use super::*;
use crate::failure::{BackendFault, CleanupCause};
use crate::release::Cleaned;
use core::{
    cell::Cell,
    pin::pin,
    task::{Context, Poll},
};
use oer_portable_time_virtual::VirtualClock;

/// A removal receipt whose end the test observes: each drop counts once.
pub(crate) struct Receipt<'a>(pub(crate) &'a Cell<u8>);

impl Drop for Receipt<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

/// How a stopped radio of the test leaves.
#[derive(Clone, Copy)]
pub(crate) enum Leaves {
    Left,
    Ended(ReleaseCause),
}

/// A stopped radio of the test.
pub(crate) struct Radio(pub(crate) Leaves);

/// The release's leave of a [`Radio`].
pub(crate) async fn leave(radio: Radio) -> Leave<Radio, ReleaseCause> {
    match radio.0 {
        Leaves::Left => Leave::Left,
        Leaves::Ended(cause) => Leave::Ended(cause),
    }
}

/// A retired port of the test: its cleanup's result and receipt, and the
/// stopped radio it gives up, if one.
pub(crate) struct TestPort<'a> {
    pub(crate) cleanup: Result<(), CleanupCause>,
    pub(crate) receipt: Option<Receipt<'a>>,
    pub(crate) stopped: Option<Radio>,
}

impl<'a> Stopped for TestPort<'a> {
    type Receipt = Receipt<'a>;
    type Radio = Radio;
    fn clean(&mut self) -> Cleaned<Receipt<'a>> {
        Cleaned {
            result: self.cleanup,
            removed: self.receipt.take(),
        }
    }
    fn take_stopped(&mut self) -> Option<Radio> {
        self.stopped.take()
    }
}

pub(crate) fn ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(core::task::Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the step waits on nothing of the test"),
    }
}

type TestNews<'a> = News<u8, u8, Receipt<'a>, ()>;

/// The actor's news of the retired `port`, as its task posts it.
fn retired_news<'a>(port: &mut TestPort<'a>) -> TestNews<'a> {
    let clock = VirtualClock::<2>::new();
    ready(retired(
        port,
        false,
        &clock,
        Duration::from_millis(1),
        leave,
    ))
}

#[test]
fn a_refused_release_returns_its_receipt_through_the_news_to_the_runner_caller() {
    let refusals = [
        // No stopped radio.
        (Ok(()), None, ReleaseCause::NotStopped),
        // A posted leave that ended.
        (
            Ok(()),
            Some(Radio(Leaves::Ended(ReleaseCause::LeaveUnanswered([
                1, 2, 3, 4,
            ])))),
            ReleaseCause::LeaveUnanswered([1, 2, 3, 4]),
        ),
        // A cleanup that did not finish: nothing released.
        (
            Err(CleanupCause::Unavailable(BackendFault::Abandoned)),
            Some(Radio(Leaves::Left)),
            ReleaseCause::Cleanup(CleanupCause::Unavailable(BackendFault::Abandoned)),
        ),
    ];
    for (cleanup, stopped, cause) in refusals {
        let ends = Cell::new(0);
        let mut port = TestPort {
            cleanup,
            receipt: Some(Receipt(&ends)),
            stopped,
        };
        let news = retired_news(&mut port);
        let (exit, returned) = ended(news);
        assert_eq!(exit, StationExitOf::ReleaseFailed(cause));
        assert_eq!(ends.get(), 0, "the refusal ends nothing");
        assert!(returned.is_some(), "the caller holds the receipt");
        drop(returned);
        assert_eq!(ends.get(), 1, "one receipt, ended once by its holder");
    }
}

#[test]
fn a_release_that_succeeded_ends_its_receipt_with_the_retired_installation() {
    let ends = Cell::new(0);
    let mut port = TestPort {
        cleanup: Ok(()),
        receipt: Some(Receipt(&ends)),
        stopped: Some(Radio(Leaves::Left)),
    };
    let news = retired_news(&mut port);
    assert_eq!(ends.get(), 0, "the news carries the receipt");
    let (exit, returned) = ended(news);
    assert_eq!(exit, StationExitOf::Stopped);
    assert!(returned.is_none());
    assert_eq!(ends.get(), 1);
}

#[test]
fn a_refused_reinstall_returns_its_receipt_through_the_news_to_the_runner_caller() {
    // The port refused the reinstall: the cleanup's receipt comes back.
    let ends = Cell::new(0);
    let cleaned = Cleaned {
        result: Ok(()),
        removed: Some(Receipt(&ends)),
    };
    let news: TestNews<'_> = reinstalled(cleaned, || Err(ReinstallCause::Resources));
    let Err((exit, returned)) = installed(news) else {
        panic!("a refused reinstall installs nothing");
    };
    assert_eq!(
        exit,
        StationExitOf::Failed(LifecycleFailure::Reinstall(ReinstallCause::Resources))
    );
    assert_eq!(ends.get(), 0);
    assert!(returned.is_some());
    drop(returned);
    assert_eq!(ends.get(), 1);

    // The cleanup did not finish: no reinstall runs, the receipt comes
    // back with the cleanup's cause.
    let ends = Cell::new(0);
    let fault = CleanupCause::Held(BackendFault::Abandoned);
    let cleaned = Cleaned {
        result: Err(fault),
        removed: Some(Receipt(&ends)),
    };
    let news: TestNews<'_> = reinstalled(cleaned, || panic!("a cleanup that failed reinstalls"));
    let Err((exit, returned)) = installed(news) else {
        panic!("a failed cleanup installs nothing");
    };
    assert_eq!(
        exit,
        StationExitOf::Failed(LifecycleFailure::Reinstall(ReinstallCause::Cleanup(fault)))
    );
    assert_eq!(ends.get(), 0);
    assert!(returned.is_some());
}

#[test]
fn a_reinstall_that_succeeded_ends_its_receipt_and_installs() {
    let ends = Cell::new(0);
    let cleaned = Cleaned {
        result: Ok(()),
        removed: Some(Receipt(&ends)),
    };
    let news: TestNews<'_> = reinstalled(cleaned, || Ok((7, 9)));
    assert_eq!(ends.get(), 1, "the retired installation's receipt ended");
    assert!(matches!(installed(news), Ok((7, 9))));
}

#[test]
fn news_out_of_order_keeps_its_receipt() {
    // A release while the station waits for an installation.
    let ends = Cell::new(0);
    let news: TestNews<'_> = News::Released {
        result: Err(ReleaseCause::NotStopped),
        network_lost: false,
        returned: Some(Receipt(&ends)),
    };
    let Err((exit, returned)) = installed(news) else {
        panic!("a release installs nothing");
    };
    assert_eq!(exit, StationExitOf::Failed(LifecycleFailure::Mailbox));
    assert!(returned.is_some());
    // A refused reinstall while the station waits for the release.
    let news: TestNews<'_> = News::Reinstall {
        cause: ReinstallCause::Epochs,
        returned: Some(Receipt(&ends)),
    };
    let (exit, kept) = ended(news);
    assert_eq!(exit, StationExitOf::Failed(LifecycleFailure::Mailbox));
    assert!(kept.is_some());
    assert_eq!(ends.get(), 0);
}
