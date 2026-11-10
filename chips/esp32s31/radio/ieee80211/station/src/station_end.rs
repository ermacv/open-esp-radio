//! How a station's installation ends on the datapath actor's task (a
//! completed retire, or an unfinished installation the task returns whole),
//! what that task tells the station's task, and how the station reads that
//! news into its exit. Generic over the actor ([`Installation`]), the
//! installation's commands `C`, tokens `K` and start refusal `E`: the
//! chip's binding names them; a host test names its own.

use core::future::Future;

use oer_portable_time::{Duration, Timer};

use crate::failure::{LifecycleFailure, ReinstallCause, ReleaseCause, Unavailable};
use crate::release::{Cleaned, Leave, Stopped};
use crate::unfinished::{Phase, Unfinished};

/// How the station ended: an ordinary stop, a refused start, a stop whose
/// radio release failed, or a failure of the lifecycle or the actor under
/// it, each typed: nothing resets, and an installation that did not retire
/// comes back whole, both halves of it. A restart of the station never ends
/// it. `E` is the start's refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationExitOf<E> {
    Stopped,
    /// Stopped and released, every owner back, but the network's stop
    /// participant was lost, so the network never confirmed its stop: a
    /// software error this exit records. The radio's stop was confirmed;
    /// nothing resets for it.
    StoppedNetworkLost,
    /// The lifecycle or the datapath actor under it ended on a failure:
    /// the installation comes back whole unless the radio's release after
    /// a refused start completed.
    Failed(LifecycleFailure),
    /// Stopped, but the Wi-Fi radio release did not complete: the stop is
    /// confirmed and nothing resets; a posted leave is never posted again.
    ReleaseFailed(ReleaseCause),
    /// The station never started: its refusal, and how the owners it
    /// returned ended.
    Unavailable(Unavailable<E>),
}

/// The datapath actor of an installation, which owns its port: the chip's
/// actor over the S31 port, an actor of the test on the host.
pub(crate) trait Installation {
    /// Its port, whose stopped radio the release takes.
    type Port: Stopped;
    /// An item a port call could not keep.
    type Unkept;
    /// An attempt a submission returned unaccepted beside such an item.
    type Unaccepted;
    /// Its port.
    fn port_mut(&mut self) -> &mut Self::Port;
    /// The item a port call could not keep, once.
    fn take_unkept(&mut self) -> Option<Self::Unkept>;
    /// The attempt a submission returned unaccepted, once.
    fn take_unaccepted(&mut self) -> Option<Self::Unaccepted>;
    /// How far its radio came when its run ended.
    fn radio_phase(&self) -> oer_protocols_ieee80211_runtime::datapath::RadioPhase;
}

/// Why a station's installation did not retire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathCause {
    /// The datapath actor ended on its fault: the lifecycle hears it and
    /// ends with it.
    Faulted(LifecycleFailure),
    /// The port did not reinstall after the station's restart.
    Reinstall(ReinstallCause),
    /// The radio's release did not complete.
    Release(ReleaseCause),
    /// The station's lifecycle ended on its failure without retiring the
    /// actor (a stop or retune proved nothing, a broken contract, news out
    /// of order): the actor's task ends with the installation.
    Lifecycle(LifecycleFailure),
}

/// A station's installation that did not retire: its actor `A` with the
/// port, the item a port call could not keep, an attempt returned
/// unaccepted, the cleanup's receipt, the cause and the phase.
pub type UnfinishedStation<A, I, U, R> = Unfinished<A, Option<I>, Option<U>, R, DatapathCause>;

/// The unfinished installation of the actor `A`.
pub(crate) type UnfinishedOf<A> = UnfinishedStation<
    A,
    <A as Installation>::Unkept,
    <A as Installation>::Unaccepted,
    <<A as Installation>::Port as Stopped>::Receipt,
>;

/// `actor`'s installation, ended for `cause` in `phase` with the cleanup's
/// `receipt`, whole: the items the actor holds come out beside it.
pub(crate) fn unfinished<A: Installation>(
    mut actor: A,
    receipt: Option<<A::Port as Stopped>::Receipt>,
    cause: DatapathCause,
    phase: Phase,
) -> UnfinishedOf<A> {
    let unkept = actor.take_unkept();
    let unaccepted = actor.take_unaccepted();
    Unfinished {
        owner: actor,
        unkept,
        unaccepted,
        receipt,
        cause,
        phase,
    }
}

/// `actor`'s installation, ended for `cause` before its retire (its fault,
/// or the lifecycle's end without retiring it), whole, in the phase its
/// radio read: a stop asked for and not confirmed is
/// [`Phase::Stopping`], whatever ended the run.
pub(crate) fn ended_running<A: Installation>(actor: A, cause: DatapathCause) -> UnfinishedOf<A> {
    let phase = Phase::of_radio(actor.radio_phase());
    unfinished(actor, None, cause, phase)
}

/// What the actor's task tells the station's task, in order, through the
/// one-slot installation mailbox. The owners stay with the actor's task:
/// an installation that does not retire is its to return
/// ([`UnfinishedStation`]).
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the chip's composition builds the other variants")
)]
pub(crate) enum News<C, K, E> {
    /// An installation, on first install and after every reinstall: its
    /// commands' issuer and the lifecycle's tokens.
    Installed(C, K),
    /// The first installation did not come up.
    Unavailable(Unavailable<E>),
    /// The port did not reinstall after the station's restart.
    Reinstall(ReinstallCause),
    /// The actor ended on its fault instead of the retire or reinstall
    /// the lifecycle asked for: its installation comes back unfinished.
    Faulted(LifecycleFailure),
    /// The radio's release after the lifecycle retired the port, and
    /// whether the last stop's network participant was lost.
    Released {
        result: Result<(), ReleaseCause>,
        network_lost: bool,
    },
    /// The actor's run stopped on the lifecycle's abandon: it produces
    /// nothing more into the queues between them.
    Abandoned,
}

/// The proof that a station installation's datapath half retired: its
/// radio's release completed and every owner of that half ended. Only
/// [`retired`] makes it, once per completed release; it lets the
/// lifecycle's half finish without a release of its own
/// ([`UnfinishedLifecycleOf::finish`]).
#[must_use = "the proof finishes the lifecycle's half of the retired installation"]
pub struct DatapathRetired(());

impl core::fmt::Debug for DatapathRetired {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("DatapathRetired")
    }
}

/// What a station installation's first stage did not take, returned whole:
/// the installation registry, once it was taken, and the minted
/// installation, once one was minted. A refused stage hands back every
/// input it was given.
#[must_use = "a refusal before installation returns the registry and the minted installation"]
pub struct NotInstalledOf<R, M> {
    /// The installation registry.
    pub installations: Option<R>,
    /// The minted installation.
    pub minted: Option<M>,
}

/// The first installation of `installations`, minted by `first`, or the
/// registry back whole when it mints none.
#[allow(
    clippy::result_large_err,
    reason = "the refusal returns the registry by value"
)]
pub(crate) fn first_installation<R, M>(
    mut installations: R,
    first: impl FnOnce(&mut R) -> Option<M>,
) -> Result<(R, M), NotInstalledOf<R, M>> {
    match first(&mut installations) {
        Some(minted) => Ok((installations, minted)),
        None => Err(NotInstalledOf {
            installations: Some(installations),
            minted: None,
        }),
    }
}

/// Run the actor's `run` until it ends or the lifecycle abandons it
/// (`abandoned`). An abandon drops the run first, so the actor produces
/// nothing more into the queues between them, and only then tells the
/// lifecycle (`mailbox`, [`News::Abandoned`]), which waits for it before
/// its own ends go out ([`abandon`]).
///
/// # Errors
///
/// The lifecycle's failure, once the run stopped and the news was posted.
pub(crate) async fn run_until_abandoned<F: Future, C, K, E>(
    run: F,
    abandoned: &mut oer_portable_memory::WakingSpscReceiver<'_, LifecycleFailure, 1>,
    mailbox: &mut oer_portable_memory::WakingSpscSender<'_, News<C, K, E>, 1>,
) -> Result<F::Output, LifecycleFailure> {
    let failure = {
        let abandon = async {
            loop {
                if let Some(failure) = abandoned.try_receive() {
                    return failure;
                }
                abandoned.ready().await;
            }
        };
        match embassy_futures::select::select(run, abandon).await {
            embassy_futures::select::Either::First(end) => return Ok(end),
            embassy_futures::select::Either::Second(failure) => failure,
        }
    };
    // The run is dropped here: the news follows its last poll.
    mailbox.ready().await.send(News::Abandoned);
    Err(failure)
}

/// Tell the actor's task once that the lifecycle ended on `failure` without
/// retiring it, and wait until its run stopped: its next news is in
/// `mailbox` ([`News::Abandoned`], or the fault it ended on first), left
/// there for the lifecycle's half. Only then do the lifecycle's ends of the
/// queues go out, so no producer runs while they are collected.
pub(crate) async fn abandon<C, K, E>(
    failure: LifecycleFailure,
    abandon: &mut oer_portable_memory::WakingSpscSender<'_, LifecycleFailure, 1>,
    mailbox: &mut oer_portable_memory::WakingSpscReceiver<'_, News<C, K, E>, 1>,
) {
    abandon.ready().await.send(failure);
    while mailbox.is_empty() {
        mailbox.ready().await;
    }
}

/// The station lifecycle's half of an installation that did not retire,
/// whole: its retired lifecycle `R`, its ends `Q` of the queues to the
/// actor with every owner in transit in them, in order, and the actor
/// task's news that came after the lifecycle stopped waiting for it.
#[must_use = "the lifecycle's half holds the station's owners of the installation"]
pub struct UnfinishedLifecycleOf<'a, R, Q, C, K, E> {
    retired: R,
    queues: Q,
    mailbox: oer_portable_memory::WakingSpscReceiver<'a, News<C, K, E>, 1>,
    /// The late news's exit, once read.
    late: Option<StationExitOf<E>>,
    /// An installation's commands and tokens a late news carried, kept.
    installed: Option<(C, K)>,
}

impl<'a, R, Q, C, K, E: Copy> UnfinishedLifecycleOf<'a, R, Q, C, K, E> {
    /// The half of `retired`, with the lifecycle's `queues` and `mailbox`.
    pub(crate) const fn new(
        retired: R,
        queues: Q,
        mailbox: oer_portable_memory::WakingSpscReceiver<'a, News<C, K, E>, 1>,
    ) -> Self {
        Self {
            retired,
            queues,
            mailbox,
            late: None,
            installed: None,
        }
    }

    /// The retired lifecycle.
    pub(crate) fn retired_mut(&mut self) -> &mut R {
        &mut self.retired
    }

    /// The lifecycle's ends of the queues to the actor: the owners in
    /// transit in them, each once, in order.
    pub fn queues(&mut self) -> &mut Q {
        &mut self.queues
    }

    /// How the actor's task ended after the lifecycle stopped waiting for
    /// it (a release past the drain's deadline, the stop after an
    /// abandon), read once from its news and kept: `None` while it has
    /// told nothing. A news of an installation is out of order
    /// ([`LifecycleFailure::Mailbox`]); its owners stay in the half.
    pub fn late_end(&mut self) -> Option<StationExitOf<E>> {
        if self.late.is_none() {
            self.late = self.mailbox.try_receive().map(|news| match news {
                News::Installed(commands, tokens) => {
                    self.installed = Some((commands, tokens));
                    StationExitOf::Failed(LifecycleFailure::Mailbox)
                }
                news => ended(news),
            });
        }
        self.late
    }

    /// Finish the half of an installation whose datapath half retired: its
    /// owners end with the retired installation, as after a completed
    /// retire, and no release runs again (the datapath half's release
    /// was the installation's one). The actual end of the actor's task is
    /// returned ([`Self::late_end`]), beside the exit the lifecycle's task
    /// returned.
    pub fn finish(mut self, retired: DatapathRetired) -> Option<StationExitOf<E>> {
        let DatapathRetired(()) = retired;
        self.late_end()
    }
}

/// A restart of `actor`'s installation: the stopped port's cleanup first (a
/// port it did not finish keeps its owners and does not reinstall), then
/// `reinstall`. A refusal of either leaves the installation unfinished,
/// the cleanup's receipt with it, and tells the refusal; after a reinstall
/// that succeeded the receipt ends, as its owners are the retired
/// installation's, and the actor runs on.
#[allow(
    clippy::type_complexity,
    clippy::result_large_err,
    reason = "the running actor and its news, or the news and the unfinished installation, by value"
)]
pub(crate) fn reinstalled<A: Installation, C, K, E>(
    mut actor: A,
    reinstall: impl FnOnce(&mut A) -> Result<(C, K), ReinstallCause>,
) -> Result<(A, News<C, K, E>), (News<C, K, E>, UnfinishedOf<A>)> {
    let Cleaned { result, removed } = actor.port_mut().clean();
    let refused = match result {
        Err(cause) => ReinstallCause::Cleanup(cause),
        Ok(()) => match reinstall(&mut actor) {
            Ok((commands, tokens)) => {
                drop(removed);
                return Ok((actor, News::Installed(commands, tokens)));
            }
            Err(cause) => cause,
        },
    };
    Err((
        News::Reinstall(refused),
        unfinished(
            actor,
            removed,
            DatapathCause::Reinstall(refused),
            Phase::Reinstalling,
        ),
    ))
}

/// The news of `actor`'s retired port, and its installation unless it
/// retired. An item a port call could not keep skips the release
/// ([`ReleaseCause::Unkept`], [`Phase::Stopped`]); otherwise the port's
/// cleanup and its radio's release run ([`crate::release::release`]). Only
/// a release that succeeded retires the installation: the actor and the
/// cleanup's receipt end with it, and the proof of it is returned
/// ([`DatapathRetired`]). A refusal leaves it unfinished, whole, in
/// [`Phase::Releasing`].
pub(crate) async fn retired<A: Installation, C, K, E, F>(
    mut actor: A,
    network_lost: bool,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(<A::Port as Stopped>::Radio) -> F,
) -> (News<C, K, E>, Result<DatapathRetired, UnfinishedOf<A>>)
where
    F: Future<Output = Leave<<A::Port as Stopped>::Radio, ReleaseCause>>,
{
    if let Some(item) = actor.take_unkept() {
        let cause = ReleaseCause::Unkept;
        let mut left = unfinished(actor, None, DatapathCause::Release(cause), Phase::Stopped);
        left.unkept = Some(item);
        return (
            News::Released {
                result: Err(cause),
                network_lost,
            },
            Err(left),
        );
    }
    let (result, returned) =
        crate::release::release(actor.port_mut(), timer, retry, leave, |_| {}).await;
    let left = match result {
        Ok(()) => {
            drop(returned);
            drop(actor);
            Ok(DatapathRetired(()))
        }
        Err(cause) => Err(unfinished(
            actor,
            returned,
            DatapathCause::Release(cause),
            Phase::Releasing,
        )),
    };
    (
        News::Released {
            result,
            network_lost,
        },
        left,
    )
}

/// The news the station waits for before a lifecycle: an installation, or
/// the exit it ends with.
pub(crate) fn installed<C, K, E>(news: News<C, K, E>) -> Result<(C, K), StationExitOf<E>> {
    match news {
        News::Installed(commands, tokens) => Ok((commands, tokens)),
        News::Unavailable(error) => Err(StationExitOf::Unavailable(error)),
        News::Reinstall(cause) => Err(StationExitOf::Failed(LifecycleFailure::Reinstall(cause))),
        News::Faulted(failure) => Err(StationExitOf::Failed(failure)),
        News::Released { .. } | News::Abandoned => {
            Err(StationExitOf::Failed(LifecycleFailure::Mailbox))
        }
    }
}

/// Whether the station lifecycle's half of an installation ended with
/// the retired installation, or goes out beside its unfinished datapath
/// half.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LifecycleHalf {
    /// The radio's release completed: the half ends, and its place may be
    /// reused.
    Retired,
    /// No release completed: the half goes out whole.
    Unfinished,
}

/// The exit once the actor's task told how the radio's release ended
/// (`None`: the stop's drain passed before it did), and how the
/// lifecycle's half settles: only a release that succeeded retires it.
/// `failure` is the lifecycle's own cause when it retired the actor after
/// a refused start; the exit keeps it.
pub(crate) fn release_settled<C, K, E>(
    news: Option<News<C, K, E>>,
    failure: Option<LifecycleFailure>,
) -> (StationExitOf<E>, LifecycleHalf) {
    let exit = match news {
        Some(news) => ended(news),
        None => StationExitOf::ReleaseFailed(ReleaseCause::Deadline),
    };
    let half = match exit {
        StationExitOf::Stopped | StationExitOf::StoppedNetworkLost => LifecycleHalf::Retired,
        _ => LifecycleHalf::Unfinished,
    };
    (failure.map_or(exit, StationExitOf::Failed), half)
}

/// The exit the actor task's last news gives.
pub(crate) fn ended<C, K, E>(news: News<C, K, E>) -> StationExitOf<E> {
    match news {
        News::Released {
            result,
            network_lost,
        } => released_exit(result, network_lost),
        News::Faulted(failure) => StationExitOf::Failed(failure),
        News::Reinstall(_) | News::Installed(..) | News::Unavailable(_) | News::Abandoned => {
            StationExitOf::Failed(LifecycleFailure::Mailbox)
        }
    }
}

/// The exit of a released radio: its release's result, and the loss of the
/// last stop's network participant, which a stopped exit records.
fn released_exit<E>(result: Result<(), ReleaseCause>, network_lost: bool) -> StationExitOf<E> {
    match (result, network_lost) {
        (Ok(()), false) => StationExitOf::Stopped,
        (Ok(()), true) => StationExitOf::StoppedNetworkLost,
        (Err(cause), _) => StationExitOf::ReleaseFailed(cause),
    }
}

#[cfg(test)]
pub(crate) mod tests;
