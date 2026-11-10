//! The station's news path over a port of the test: the actor side's
//! [`reinstalled`] and [`retired`], read by the station's [`installed`] and
//! [`ended`], as the chip's tasks call them.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use super::*;
use crate::failure::{BackendFault, CleanupCause};
use crate::release::Cleaned;
use crate::unfinished::Phase;
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

type TestNews = News<u8, u8, ()>;

/// A datapath actor of the test: its port, another owner it holds, and the
/// item and attempt its port's calls returned.
pub(crate) struct TestActor<'a> {
    pub(crate) port: TestPort<'a>,
    pub(crate) other: Option<Receipt<'a>>,
    pub(crate) unkept: Option<Receipt<'a>>,
    pub(crate) unaccepted: Option<Receipt<'a>>,
    pub(crate) phase: oer_protocols_ieee80211_runtime::datapath::RadioPhase,
}

impl<'a> Installation for TestActor<'a> {
    type Port = TestPort<'a>;
    type Unkept = Receipt<'a>;
    type Unaccepted = Receipt<'a>;
    fn port_mut(&mut self) -> &mut TestPort<'a> {
        &mut self.port
    }
    fn take_unkept(&mut self) -> Option<Receipt<'a>> {
        self.unkept.take()
    }
    fn take_unaccepted(&mut self) -> Option<Receipt<'a>> {
        self.unaccepted.take()
    }
    fn radio_phase(&self) -> oer_protocols_ieee80211_runtime::datapath::RadioPhase {
        self.phase
    }
}

/// Counters of every owner of a test installation: the cleanup's receipt,
/// the actor's other owner, its unkept item and its unaccepted attempt.
#[derive(Default)]
struct Ends {
    receipt: Cell<u8>,
    other: Cell<u8>,
    unkept: Cell<u8>,
    unaccepted: Cell<u8>,
}

impl Ends {
    fn all(&self) -> [u8; 4] {
        [
            self.receipt.get(),
            self.other.get(),
            self.unkept.get(),
            self.unaccepted.get(),
        ]
    }
}

/// An actor over a port of `cleanup` and `stopped`, every owner counted by
/// `ends`; `items` gives it an unkept item and an unaccepted attempt.
fn actor<'a>(
    ends: &'a Ends,
    cleanup: Result<(), CleanupCause>,
    stopped: Option<Radio>,
    items: bool,
) -> TestActor<'a> {
    TestActor {
        port: TestPort {
            cleanup,
            receipt: Some(Receipt(&ends.receipt)),
            stopped,
        },
        other: Some(Receipt(&ends.other)),
        unkept: items.then(|| Receipt(&ends.unkept)),
        unaccepted: items.then(|| Receipt(&ends.unaccepted)),
        phase: oer_protocols_ieee80211_runtime::datapath::RadioPhase::Running,
    }
}

/// The actor task's end of the retired `actor`, as it calls it.
fn retired_of(actor: TestActor<'_>) -> (TestNews, Option<UnfinishedOf<TestActor<'_>>>) {
    let clock = VirtualClock::<2>::new();
    ready(retired(
        actor,
        false,
        &clock,
        Duration::from_millis(1),
        leave,
    ))
}

#[test]
fn a_refused_release_returns_the_whole_installation_to_the_actor_task_caller() {
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
        let ends = Ends::default();
        let (news, left) = retired_of(actor(&ends, cleanup, stopped, false));
        assert_eq!(ended(news), StationExitOf::ReleaseFailed(cause));
        assert_eq!(ends.all(), [0; 4], "the refusal ends nothing");
        let Some(left) = left else {
            panic!("a refused release leaves the installation unfinished");
        };
        assert_eq!(left.cause, DatapathCause::Release(cause));
        assert_eq!(left.phase, Phase::Releasing);
        assert!(left.receipt.is_some(), "the caller holds the receipt");
        assert!(left.owner.other.is_some(), "and the actor with its owners");
        drop(left);
        assert_eq!(ends.all(), [1, 1, 0, 0], "each ended once by its holder");
    }
}

#[test]
fn a_release_that_succeeded_retires_the_whole_installation() {
    let ends = Ends::default();
    let (news, left) = retired_of(actor(&ends, Ok(()), Some(Radio(Leaves::Left)), false));
    assert!(left.is_none(), "a completed retire");
    assert_eq!(ended(news), StationExitOf::Stopped);
    assert_eq!(ends.all(), [1, 1, 0, 0]);
}

#[test]
fn an_item_the_actor_could_not_keep_skips_the_release_and_comes_back_whole() {
    let ends = Ends::default();
    let (news, left) = retired_of(actor(&ends, Ok(()), Some(Radio(Leaves::Left)), true));
    assert_eq!(
        ended(news),
        StationExitOf::ReleaseFailed(ReleaseCause::Unkept)
    );
    let Some(mut left) = left else {
        panic!("the item leaves the installation unfinished");
    };
    assert_eq!(left.phase, Phase::Stopped);
    assert!(left.unkept.is_some() && left.unaccepted.is_some());
    assert!(left.receipt.is_none(), "no cleanup ran");
    assert!(
        left.owner.port.stopped.is_some(),
        "no release took the radio"
    );
    assert!(left.owner.port.receipt.take().is_some());
    assert_eq!(ends.all(), [1, 0, 0, 0]);
    drop(left);
    assert_eq!(ends.all(), [1, 1, 1, 1]);
}

#[test]
fn a_refused_reinstall_returns_the_whole_installation_to_the_actor_task_caller() {
    // The port refused the reinstall: the installation comes back, the
    // cleanup's receipt with it.
    let ends = Ends::default();
    let Err((news, left)) = reinstalled::<_, u8, u8, ()>(actor(&ends, Ok(()), None, false), |_| {
        Err(ReinstallCause::Resources)
    }) else {
        panic!("a refused reinstall installs nothing");
    };
    assert_eq!(
        installed(news).err(),
        Some(StationExitOf::Failed(LifecycleFailure::Reinstall(
            ReinstallCause::Resources
        )))
    );
    assert_eq!(left.phase, Phase::Reinstalling);
    assert_eq!(
        left.cause,
        DatapathCause::Reinstall(ReinstallCause::Resources)
    );
    assert_eq!(ends.all(), [0; 4]);
    assert!(left.receipt.is_some() && left.owner.other.is_some());
    drop(left);
    assert_eq!(ends.all(), [1, 1, 0, 0]);

    // The cleanup did not finish: no reinstall runs, and the installation
    // comes back with the cleanup's cause.
    let ends = Ends::default();
    let fault = CleanupCause::Held(BackendFault::Abandoned);
    let Err((news, left)) =
        reinstalled::<_, u8, u8, ()>(actor(&ends, Err(fault), None, true), |_| {
            panic!("a cleanup that failed reinstalls")
        })
    else {
        panic!("a failed cleanup installs nothing");
    };
    assert_eq!(
        installed(news).err(),
        Some(StationExitOf::Failed(LifecycleFailure::Reinstall(
            ReinstallCause::Cleanup(fault)
        )))
    );
    assert!(left.receipt.is_some() && left.unkept.is_some() && left.unaccepted.is_some());
    assert_eq!(ends.all(), [0; 4]);
}

#[test]
fn a_reinstall_that_succeeded_ends_its_receipt_and_the_actor_runs_on() {
    let ends = Ends::default();
    let Ok((actor, news)) =
        reinstalled::<_, u8, u8, ()>(actor(&ends, Ok(()), None, false), |_| Ok((7, 9)))
    else {
        panic!("the reinstall succeeds");
    };
    assert_eq!(
        ends.all(),
        [1, 0, 0, 0],
        "the retired installation's receipt ended"
    );
    assert!(matches!(installed(news), Ok((7, 9))));
    assert!(actor.other.is_some(), "the actor runs on with its owners");
}

#[test]
fn a_faulted_actor_returns_every_owner_beside_its_cause() {
    let ends = Ends::default();
    let left = unfinished(
        actor(&ends, Ok(()), None, true),
        None,
        DatapathCause::Faulted(LifecycleFailure::Unkept),
        Phase::Running,
    );
    assert!(left.unkept.is_some() && left.unaccepted.is_some());
    assert!(left.owner.unkept.is_none(), "each item comes out once");
    assert_eq!(ends.all(), [0; 4]);
    drop(left);
    assert_eq!(ends.all(), [1, 1, 1, 1]);
}

#[test]
fn news_out_of_order_names_the_mailbox() {
    let news: TestNews = News::Released {
        result: Err(ReleaseCause::NotStopped),
        network_lost: false,
    };
    assert_eq!(
        installed(news).err(),
        Some(StationExitOf::Failed(LifecycleFailure::Mailbox))
    );
    let news: TestNews = News::Reinstall(ReinstallCause::Epochs);
    assert_eq!(
        ended(news),
        StationExitOf::Failed(LifecycleFailure::Mailbox)
    );
}

/// The owner's case: the actor ended on an item a port call could not keep
/// while the stop the lifecycle asked for waited. The installation comes
/// back whole, in the phase the radio read (stopping), not as a running
/// radio.
#[test]
fn a_fault_during_a_stop_returns_the_installation_in_the_stopping_phase() {
    use oer_protocols_ieee80211_runtime::datapath::RadioPhase;
    let cases = [
        (RadioPhase::Stopping, Phase::Stopping),
        (RadioPhase::Running, Phase::Running),
        (RadioPhase::Stopped, Phase::Stopped),
    ];
    for (radio, phase) in cases {
        let ends = Ends::default();
        let mut running = actor(&ends, Ok(()), Some(Radio(Leaves::Left)), true);
        running.phase = radio;
        let left = ended_running(running, DatapathCause::Faulted(LifecycleFailure::Unkept));
        assert_eq!(left.phase, phase);
        assert_eq!(left.cause, DatapathCause::Faulted(LifecycleFailure::Unkept));
        assert!(left.unkept.is_some() && left.unaccepted.is_some());
        assert_eq!(ends.all(), [0; 4], "nothing ended on the way");
        drop(left);
        assert_eq!(ends.all(), [1, 1, 1, 1]);
    }
}

/// Every failure that once reset the chip ends the actor's run with the
/// installation whole: the actor with its port and every owner, the items,
/// the cause and the radio's phase, for the application.
#[test]
fn each_former_resetting_failure_returns_the_whole_installation() {
    use oer_protocols_ieee80211_contracts_port::StartRefused;
    use oer_protocols_ieee80211_runtime::datapath::{ActorViolation, RadioPhase};
    use oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation;
    let failures = [
        LifecycleFailure::StartRefused(StartRefused::Deadline),
        LifecycleFailure::Port(BackendFault::Abandoned),
        LifecycleFailure::Actor(ActorViolation::PairwiseCustody),
        LifecycleFailure::Contract(LifecycleViolation::Missing),
        LifecycleFailure::Mailbox,
        LifecycleFailure::Unkept,
    ];
    for failure in failures {
        for cause in [
            DatapathCause::Faulted(failure),
            DatapathCause::Lifecycle(failure),
        ] {
            let ends = Ends::default();
            let mut running = actor(&ends, Ok(()), Some(Radio(Leaves::Left)), true);
            running.phase = RadioPhase::Stopping;
            let left = ended_running(running, cause);
            assert_eq!(left.cause, cause);
            assert_eq!(left.phase, Phase::Stopping);
            assert!(left.owner.other.is_some() && left.owner.port.receipt.is_some());
            assert!(left.unkept.is_some() && left.unaccepted.is_some());
            assert_eq!(ends.all(), [0; 4], "{failure:?}: nothing ended on the way");
            drop(left);
            assert_eq!(
                ends.all(),
                [1, 1, 1, 1],
                "{failure:?}: each once, by its holder"
            );
        }
    }
}

/// The lifecycle's half after its actor retired: only a release that
/// succeeded retires it; a refused or unanswered release, the drain's end
/// first, or news out of order send it out with the datapath half. A
/// refused start's exit keeps its cause either way.
#[test]
fn only_a_completed_release_retires_the_lifecycle_half() {
    use oer_protocols_ieee80211_contracts_port::StartRefused;
    let released = |result, network_lost| {
        Some(TestNews::Released {
            result,
            network_lost,
        })
    };
    assert_eq!(
        release_settled(released(Ok(()), false), None),
        (StationExitOf::Stopped, LifecycleHalf::Retired)
    );
    assert_eq!(
        release_settled(released(Ok(()), true), None),
        (StationExitOf::StoppedNetworkLost, LifecycleHalf::Retired)
    );
    assert_eq!(
        release_settled(released(Err(ReleaseCause::NotStopped), false), None),
        (
            StationExitOf::ReleaseFailed(ReleaseCause::NotStopped),
            LifecycleHalf::Unfinished
        )
    );
    assert_eq!(
        release_settled::<u8, u8, ()>(None, None),
        (
            StationExitOf::ReleaseFailed(ReleaseCause::Deadline),
            LifecycleHalf::Unfinished
        )
    );
    assert_eq!(
        release_settled(Some(TestNews::Reinstall(ReinstallCause::Epochs)), None),
        (
            StationExitOf::Failed(LifecycleFailure::Mailbox),
            LifecycleHalf::Unfinished
        )
    );
    let refused = LifecycleFailure::StartRefused(StartRefused::Deadline);
    assert_eq!(
        release_settled(released(Ok(()), false), Some(refused)),
        (StationExitOf::Failed(refused), LifecycleHalf::Retired)
    );
    assert_eq!(
        release_settled(
            released(Err(ReleaseCause::NotStopped), false),
            Some(refused)
        ),
        (StationExitOf::Failed(refused), LifecycleHalf::Unfinished)
    );
}

/// The lifecycle's half leaves its place before the place is reused: moved
/// to its home, the place takes the next installation's lifecycle (a half
/// still in the place keeps it borrowed: the `compile_fail` example of
/// [`crate::unfinished`]).
#[test]
fn an_unfinished_half_moves_home_and_frees_its_place() {
    use core::mem::MaybeUninit;
    use oer_portable_memory::Placed;
    let ends = Ends::default();
    let mut place = MaybeUninit::uninit();
    let mut home = MaybeUninit::uninit();
    let half = Placed::init(&mut place, |p| p.write(Receipt(&ends.other)));
    let kept = half.move_to(&mut home);
    let next = Placed::init(&mut place, |p| p.write(Receipt(&ends.receipt)));
    assert_eq!(ends.all(), [0; 4], "the moved half ends nothing");
    drop(next);
    assert_eq!(ends.all(), [1, 0, 0, 0]);
    drop(kept);
    assert_eq!(
        ends.all(),
        [1, 1, 0, 0],
        "the half ends once, with its holder"
    );
}
