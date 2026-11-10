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
/// cleanup's receipt end with it. A refusal leaves it unfinished, whole,
/// in [`Phase::Releasing`].
pub(crate) async fn retired<A: Installation, C, K, E, F>(
    mut actor: A,
    network_lost: bool,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(<A::Port as Stopped>::Radio) -> F,
) -> (News<C, K, E>, Option<UnfinishedOf<A>>)
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
            Some(left),
        );
    }
    let (result, returned) =
        crate::release::release(actor.port_mut(), timer, retry, leave, |_| {}).await;
    let left = match result {
        Ok(()) => {
            drop(returned);
            drop(actor);
            None
        }
        Err(cause) => Some(unfinished(
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
        News::Released { .. } => Err(StationExitOf::Failed(LifecycleFailure::Mailbox)),
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
        News::Reinstall(_) | News::Installed(..) | News::Unavailable(_) => {
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
