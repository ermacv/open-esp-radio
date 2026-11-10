//! The station's news path over a port of the test: the actor side's
//! [`reinstalled`] and [`retired`], read by the station's [`installed`] and
//! [`ended`], as the chip's tasks call them.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use super::*;
use crate::failure::{BackendFault, CleanupCause};
use crate::release::{Cleaned, Unreleased};
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
    pub(crate) stopped: Result<Radio, Unreleased>,
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
    fn take_stopped(&mut self) -> Result<Radio, Unreleased> {
        core::mem::replace(&mut self.stopped, Err(Unreleased::Held))
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

/// The slot identity of the test's installations.
pub(crate) struct TestId;

type TestNews = News<u8, u8, (), TestId>;

/// The test identity's installation of `generation`.
const fn installation(generation: u32) -> InstallationOf<TestId> {
    InstallationOf::minted(generation)
}

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
    stopped: Result<Radio, Unreleased>,
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

/// The actor task's end of the retired `actor` of the installation of
/// `generation`, as it calls it.
fn retired_of(
    actor: TestActor<'_>,
    generation: u32,
) -> (
    TestNews,
    Result<DatapathRetired<TestId>, UnfinishedOf<TestActor<'_>>>,
) {
    let clock = VirtualClock::<2>::new();
    ready(retired(
        actor,
        installation(generation),
        false,
        &clock,
        Duration::from_millis(1),
        leave,
    ))
}

#[test]
fn a_refused_release_returns_the_whole_installation_to_the_actor_task_caller() {
    let refusals = [
        // No confirmed stop.
        (
            Ok(()),
            Err(Unreleased::StopUnconfirmed),
            ReleaseCause::StopUnconfirmed,
        ),
        // A confirmed stop whose port still holds owners.
        (Ok(()), Err(Unreleased::Held), ReleaseCause::Held),
        // A posted leave that ended.
        (
            Ok(()),
            Ok(Radio(Leaves::Ended(ReleaseCause::LeaveUnanswered([
                1, 2, 3, 4,
            ])))),
            ReleaseCause::LeaveUnanswered([1, 2, 3, 4]),
        ),
        // A cleanup that did not finish: nothing released.
        (
            Err(CleanupCause::Unavailable(BackendFault::Abandoned)),
            Ok(Radio(Leaves::Left)),
            ReleaseCause::Cleanup(CleanupCause::Unavailable(BackendFault::Abandoned)),
        ),
    ];
    for (cleanup, stopped, cause) in refusals {
        let ends = Ends::default();
        let (news, left) = retired_of(actor(&ends, cleanup, stopped, false), 0);
        assert_eq!(ended(news), StationExitOf::ReleaseFailed(cause));
        assert_eq!(ends.all(), [0; 4], "the refusal ends nothing");
        let Err(left) = left else {
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
    let (news, left) = retired_of(actor(&ends, Ok(()), Ok(Radio(Leaves::Left)), false), 0);
    assert!(left.is_ok(), "a completed retire");
    assert_eq!(ended(news), StationExitOf::Stopped);
    assert_eq!(ends.all(), [1, 1, 0, 0]);
}

#[test]
fn an_item_the_actor_could_not_keep_skips_the_release_and_comes_back_whole() {
    let ends = Ends::default();
    let (news, left) = retired_of(actor(&ends, Ok(()), Ok(Radio(Leaves::Left)), true), 0);
    assert_eq!(
        ended(news),
        StationExitOf::ReleaseFailed(ReleaseCause::Unkept)
    );
    let Err(mut left) = left else {
        panic!("the item leaves the installation unfinished");
    };
    assert_eq!(left.phase, Phase::Stopped);
    assert!(left.unkept.is_some() && left.unaccepted.is_some());
    assert!(left.receipt.is_none(), "no cleanup ran");
    assert!(left.owner.port.stopped.is_ok(), "no release took the radio");
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
    let Err((news, left)) = reinstalled::<_, u8, u8, (), TestId>(
        actor(&ends, Ok(()), Err(Unreleased::StopUnconfirmed), false),
        |_| Err(ReinstallCause::Resources),
    ) else {
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
    let Err((news, left)) = reinstalled::<_, u8, u8, (), TestId>(
        actor(&ends, Err(fault), Err(Unreleased::StopUnconfirmed), true),
        |_| panic!("a cleanup that failed reinstalls"),
    ) else {
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
    let Ok((actor, news)) = reinstalled::<_, u8, u8, (), TestId>(
        actor(&ends, Ok(()), Err(Unreleased::StopUnconfirmed), false),
        |_| Ok((7, 9, installation(1))),
    ) else {
        panic!("the reinstall succeeds");
    };
    assert_eq!(
        ends.all(),
        [1, 0, 0, 0],
        "the retired installation's receipt ended"
    );
    assert!(matches!(installed(news), Ok((7, 9, next)) if next == installation(1)));
    assert!(actor.other.is_some(), "the actor runs on with its owners");
}

#[test]
fn a_faulted_actor_returns_every_owner_beside_its_cause() {
    let ends = Ends::default();
    let left = unfinished(
        actor(&ends, Ok(()), Err(Unreleased::StopUnconfirmed), true),
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
        result: Err(ReleaseCause::Held),
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
        let mut running = actor(&ends, Ok(()), Ok(Radio(Leaves::Left)), true);
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
            let mut running = actor(&ends, Ok(()), Ok(Radio(Leaves::Left)), true);
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
        release_settled(released(Err(ReleaseCause::Held), false), None),
        (
            StationExitOf::ReleaseFailed(ReleaseCause::Held),
            LifecycleHalf::Unfinished
        )
    );
    assert_eq!(
        release_settled::<u8, u8, (), TestId>(None, None),
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
        release_settled(released(Err(ReleaseCause::Held), false), Some(refused)),
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

/// An actor that ended on its fault instead of the retire or reinstall the
/// lifecycle asked for tells it: a lifecycle waiting for the release or the
/// reinstall ends with the fault, and its half goes out.
#[test]
fn a_faulted_actor_ends_the_lifecycles_wait() {
    let failure = LifecycleFailure::Unkept;
    assert_eq!(
        installed(TestNews::Faulted(failure)),
        Err(StationExitOf::Failed(failure))
    );
    assert_eq!(
        release_settled(Some(TestNews::Faulted(failure)), None),
        (StationExitOf::Failed(failure), LifecycleHalf::Unfinished)
    );
}

/// The actor's run of the test: each poll sends the next number into the
/// queue to the lifecycle; `alive` reads whether it still runs.
struct Producer<'q, 'a> {
    queue: &'q oer_portable_memory::WakingSpscSender<'a, u8, 8>,
    next: u8,
    alive: &'q Cell<bool>,
}

impl Future for Producer<'_, '_> {
    type Output = ();
    fn poll(mut self: core::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        let next = self.next;
        if self.queue.try_send(next).is_ok() {
            self.next = next.wrapping_add(1);
        }
        Poll::Pending
    }
}

impl Drop for Producer<'_, '_> {
    fn drop(&mut self) {
        self.alive.set(false);
    }
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(core::task::Waker::noop()))
}

/// A lifecycle that abandons the actor: the actor's run stops before the
/// lifecycle's ends go out, so every owner in transit is in the queues, in
/// the order it was sent, each once, and nothing follows it; the actor's
/// news that it stopped is the half's late end.
#[test]
fn an_abandon_stops_the_actor_before_the_lifecycles_ends_go_out() {
    use oer_portable_memory::WakingSpscQueue;
    let failure = LifecycleFailure::Contract(
        oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation::Missing,
    );
    let mut transit = WakingSpscQueue::<u8, 8>::new();
    let (to_lifecycle, in_transit) = transit.split();
    let mut abandon_place = WakingSpscQueue::<LifecycleFailure, 1>::new();
    let (mut abandon_end, mut abandoned) = abandon_place.split();
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mut mailbox, mut news) = mailbox_place.split();
    let alive = Cell::new(true);
    let producer = Producer {
        queue: &to_lifecycle,
        next: 0,
        alive: &alive,
    };
    let ended = {
        let mut datapath = pin!(run_until_abandoned(producer, &mut abandoned, &mut mailbox));
        for _ in 0..3 {
            assert!(poll_once(datapath.as_mut()).is_pending());
        }
        let lifecycle = async {
            let half = UnfinishedLifecycleOf::new((), (), installation(0), news)
                .abandoned(failure, &mut abandon_end)
                .await;
            // The lifecycle's ends go out here.
            (alive.get(), half)
        };
        let mut both = pin!(embassy_futures::join::join(lifecycle, datapath.as_mut()));
        let mut ended = None;
        for _ in 0..4 {
            if let Poll::Ready(both) = poll_once(both.as_mut()) {
                ended = Some(both);
                break;
            }
        }
        ended
    };
    let Some(((producing, mut half), run)) = ended else {
        panic!("both sides end");
    };
    assert!(!producing, "the actor stopped before the ends went out");
    assert_eq!(run, Err(failure));
    let mut received = heapless::Vec::<u8, 8>::new();
    while let Some(item) = in_transit.try_receive() {
        assert!(received.push(item).is_ok());
    }
    // Three polls before the abandon; the run is not polled once it is
    // queued.
    assert_eq!(received, [0, 1, 2], "in order, each once");
    assert_eq!(half.late_end(), Some(LateEnd::Abandoned));
}

/// The owner's case A: a news of an installation already in the mailbox
/// is no confirmation that the actor's run stopped. The wait goes on while
/// the abandon is still in its queue, keeps the installation's owners, and
/// ends only on the news that ends the actor's task.
#[test]
fn a_queued_installation_does_not_end_the_abandons_wait() {
    use oer_portable_memory::WakingSpscQueue;
    let failure = LifecycleFailure::Mailbox;
    let mut abandon_place = WakingSpscQueue::<LifecycleFailure, 1>::new();
    let (mut abandon_end, abandoned) = abandon_place.split();
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mailbox, news) = mailbox_place.split();
    assert!(
        mailbox
            .try_send(News::Installed(5, 6, installation(1)))
            .is_ok()
    );
    let half = UnfinishedLifecycleOf::new((), (), installation(0), news);
    let mut wait = pin!(half.abandoned(failure, &mut abandon_end));
    assert!(
        poll_once(wait.as_mut()).is_pending(),
        "an installation's news confirms nothing"
    );
    assert_eq!(
        abandoned.try_receive(),
        Some(failure),
        "the abandon is still in its queue while the wait goes on"
    );
    assert!(poll_once(wait.as_mut()).is_pending());
    assert!(mailbox.try_send(News::Abandoned).is_ok());
    let Poll::Ready(mut half) = poll_once(wait.as_mut()) else {
        panic!("the actor's end confirms the stop");
    };
    assert_eq!(half.late_end(), Some(LateEnd::Abandoned));
    assert!(
        matches!(half.take_installed(), Some((5, 6, kept)) if kept == installation(1)),
        "the installation's owners come back with the half"
    );
    assert!(half.take_installed().is_none(), "once");
}

/// Two news of installations before the end (one already in the mailbox,
/// one of a reinstall past its run) do not end the wait either: both are
/// kept, in order, and only the end of the actor's task ends it.
#[test]
fn two_installation_news_are_kept_and_only_the_end_ends_the_wait() {
    use oer_portable_memory::WakingSpscQueue;
    let mut abandon_place = WakingSpscQueue::<LifecycleFailure, 1>::new();
    let (mut abandon_end, _abandoned) = abandon_place.split();
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mailbox, news) = mailbox_place.split();
    assert!(
        mailbox
            .try_send(News::Installed(1, 2, installation(1)))
            .is_ok()
    );
    let half = UnfinishedLifecycleOf::new((), (), installation(0), news);
    let mut wait = pin!(half.abandoned(LifecycleFailure::Mailbox, &mut abandon_end));
    assert!(poll_once(wait.as_mut()).is_pending());
    assert!(
        mailbox
            .try_send(News::Installed(3, 4, installation(2)))
            .is_ok()
    );
    assert!(
        poll_once(wait.as_mut()).is_pending(),
        "a second installation's news confirms nothing"
    );
    assert!(mailbox.try_send(News::Abandoned).is_ok());
    let Poll::Ready(mut half) = poll_once(wait.as_mut()) else {
        panic!("the actor's end confirms the stop");
    };
    assert_eq!(half.late_end(), Some(LateEnd::Abandoned));
    assert!(matches!(half.take_installed(), Some((1, 2, _))));
    assert!(matches!(half.take_installed(), Some((3, 4, _))));
    assert!(half.take_installed().is_none());
}

/// An abandon already queued stops the actor's run before its first poll:
/// it produces nothing, so no further reinstall's news can follow.
#[test]
fn a_queued_abandon_stops_the_run_before_it_is_polled() {
    use oer_portable_memory::WakingSpscQueue;
    let mut transit = WakingSpscQueue::<u8, 8>::new();
    let (to_lifecycle, in_transit) = transit.split();
    let mut abandon_place = WakingSpscQueue::<LifecycleFailure, 1>::new();
    let (abandon_end, mut abandoned) = abandon_place.split();
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mut mailbox, news) = mailbox_place.split();
    let alive = Cell::new(true);
    assert!(abandon_end.try_send(LifecycleFailure::Mailbox).is_ok());
    let producer = Producer {
        queue: &to_lifecycle,
        next: 0,
        alive: &alive,
    };
    let mut datapath = pin!(run_until_abandoned(producer, &mut abandoned, &mut mailbox));
    assert_eq!(
        poll_once(datapath.as_mut()),
        Poll::Ready(Err(LifecycleFailure::Mailbox))
    );
    assert!(!alive.get());
    assert!(
        in_transit.try_receive().is_none(),
        "the run was never polled"
    );
    assert!(matches!(news.try_receive(), Some(News::Abandoned)));
}

/// The owner's case B: the proof that installation 0 retired does not
/// finish the lifecycle half of installation 1, which reused its places:
/// the half and the proof come back unchanged and nothing of the half
/// ended. The half's own proof finishes it.
#[test]
fn a_proof_of_another_installation_finishes_nothing() {
    use oer_portable_memory::WakingSpscQueue;
    let earlier = Ends::default();
    let (_, proof) = retired_of(actor(&earlier, Ok(()), Ok(Radio(Leaves::Left)), false), 0);
    let Ok(foreign) = proof else {
        panic!("installation 0 retired");
    };
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (_mailbox, news) = mailbox_place.split();
    let mut transit = WakingSpscQueue::<u8, 8>::new();
    let (to_lifecycle, in_transit) = transit.split();
    assert!(to_lifecycle.try_send(4).is_ok());
    let lifecycle = Ends::default();
    let half =
        UnfinishedLifecycleOf::new(Receipt(&lifecycle.other), in_transit, installation(1), news);
    let Err(ForeignProof {
        half: mut kept,
        proof,
    }) = half.finish(foreign)
    else {
        panic!("a proof of installation 0 finished installation 1's half");
    };
    assert_eq!(lifecycle.all(), [0; 4], "nothing of the half ended");
    assert_eq!(
        proof.installation(),
        installation(0),
        "the proof comes back"
    );
    assert_eq!(kept.installation(), installation(1));
    assert_eq!(kept.queues().try_receive(), Some(4), "its owners stay");
    let own = Ends::default();
    let (_, proof) = retired_of(actor(&own, Ok(()), Ok(Radio(Leaves::Left)), false), 1);
    let Ok(own) = proof else {
        panic!("installation 1 retired");
    };
    assert!(kept.finish(own).is_ok(), "its own proof finishes it");
    assert_eq!(lifecycle.all(), [0, 1, 0, 0]);
}

/// The stop after an abandon reads as such in the lifecycle's half, not as
/// a broken mailbox.
#[test]
fn the_stop_after_an_abandon_reads_abandoned() {
    use oer_portable_memory::WakingSpscQueue;
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mailbox, news) = mailbox_place.split();
    assert!(mailbox.try_send(News::Abandoned).is_ok());
    let mut half = UnfinishedLifecycleOf::new((), (), installation(0), news);
    assert_eq!(half.late_end(), Some(LateEnd::Abandoned));
}

/// A registry that mints no first installation comes back whole.
#[test]
fn a_registry_that_mints_nothing_comes_back_whole() {
    let ends = Ends::default();
    let Err(refused) = first_installation(Receipt(&ends.other), |_| None::<u8>) else {
        panic!("nothing was minted");
    };
    assert!(refused.minted.is_none());
    assert!(refused.installations.is_some(), "the registry is returned");
    assert_eq!(ends.all(), [0; 4]);
    drop(refused);
    assert_eq!(ends.all(), [0, 1, 0, 0], "ended once, by its holder");
    let ends = Ends::default();
    let Ok((registry, minted)) = first_installation(Receipt(&ends.other), |_| Some(7u8)) else {
        panic!("one was minted");
    };
    assert_eq!(minted, 7);
    drop(registry);
    assert_eq!(ends.all(), [0, 1, 0, 0]);
}

/// The datapath half retired after the lifecycle's drain deadline passed:
/// the lifecycle's exit keeps the deadline, the half reads the actual end
/// of the actor's task (kept once read), hands out its owners in transit,
/// and finishes on the datapath's proof of its retire, with no release of
/// its own (it holds none to run).
#[test]
fn a_late_retire_finishes_the_lifecycles_half_without_a_second_release() {
    use oer_portable_memory::WakingSpscQueue;
    let (exit, half) = release_settled::<u8, u8, (), TestId>(None, None);
    assert_eq!(exit, StationExitOf::ReleaseFailed(ReleaseCause::Deadline));
    assert_eq!(half, LifecycleHalf::Unfinished);
    let datapath = Ends::default();
    let (late, proof) = retired_of(actor(&datapath, Ok(()), Ok(Radio(Leaves::Left)), false), 0);
    let Ok(proof) = proof else {
        panic!("the release completed");
    };
    assert_eq!(datapath.all(), [1, 1, 0, 0], "the datapath half ended");
    let mut mailbox_place = WakingSpscQueue::<TestNews, 1>::new();
    let (mailbox, news) = mailbox_place.split();
    assert!(mailbox.try_send(late).is_ok());
    let mut transit = WakingSpscQueue::<u8, 8>::new();
    let (to_lifecycle, in_transit) = transit.split();
    assert!(to_lifecycle.try_send(9).is_ok());
    let lifecycle = Ends::default();
    let mut half =
        UnfinishedLifecycleOf::new(Receipt(&lifecycle.other), in_transit, installation(0), news);
    assert_eq!(half.queues().try_receive(), Some(9), "the owner in transit");
    assert_eq!(half.queues().try_receive(), None, "once");
    assert!(half.retired_mut().0.get() == 0);
    assert!(to_lifecycle.try_send(10).is_ok());
    let stopped = Some(LateEnd::Ended(StationExitOf::Stopped));
    assert_eq!(half.late_end(), stopped);
    assert_eq!(half.late_end(), stopped, "kept once read");
    assert!(half.take_installed().is_none());
    assert_eq!(lifecycle.all(), [0; 4]);
    let Ok(finished) = half.finish(proof) else {
        panic!("its own installation's proof");
    };
    assert_eq!(finished.late, stopped);
    assert_eq!(
        finished.queues.try_receive(),
        Some(10),
        "an owner still in transit comes back"
    );
    assert!(finished.installed.iter().all(Option::is_none));
    assert_eq!(
        lifecycle.all(),
        [0, 1, 0, 0],
        "the lifecycle's half ended once"
    );
}

/// One side's queue of the test.
struct Side<'q, 'a>(oer_portable_memory::WakingSpscReceiver<'q, Receipt<'a>, 4>);

impl InTransit for Side<'_, '_> {
    fn holds(&self) -> bool {
        !self.0.is_empty()
    }
}

/// A completed retire's queues: empty ones end with the retired
/// installation; one that still holds owners after its producer stopped
/// comes back whole, each owner once, in order.
#[test]
fn a_completed_retire_returns_the_owners_still_in_transit() {
    use oer_portable_memory::WakingSpscQueue;
    let ends = Ends::default();
    let mut empty = WakingSpscQueue::<Receipt<'_>, 4>::new();
    let (_producer, receiver) = empty.split();
    assert!(
        in_transit(Side(receiver)).is_none(),
        "nothing is in transit"
    );
    let mut place = WakingSpscQueue::<Receipt<'_>, 4>::new();
    let (producer, receiver) = place.split();
    assert!(producer.try_send(Receipt(&ends.receipt)).is_ok());
    assert!(producer.try_send(Receipt(&ends.other)).is_ok());
    drop(producer);
    let Some(Side(left)) = in_transit(Side(receiver)) else {
        panic!("the owners in transit come back");
    };
    assert_eq!(ends.all(), [0; 4], "nothing ended on the way");
    drop(left.try_receive());
    assert_eq!(ends.all(), [1, 0, 0, 0], "first in, first out");
    drop(left.try_receive());
    assert_eq!(ends.all(), [1, 1, 0, 0], "each once");
    assert!(left.try_receive().is_none());
}
