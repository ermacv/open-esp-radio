//! What the datapath actor's task tells the station's task after an
//! installation ended, and how the station reads that news into its exit
//! and the removal receipt it returns to the runner's caller. Generic over
//! the installation's commands `C`, tokens `K`, receipt `R` and start
//! refusal `E`: the chip's binding names them; a host test names its own.

use core::future::Future;

use oer_portable_time::{Duration, Timer};

use crate::failure::{LifecycleFailure, ReinstallCause, ReleaseCause, Unavailable};
use crate::release::{Cleaned, Leave, Stopped};

/// How the station ended: an ordinary stop, a refused start or a stop
/// whose radio release failed (typed, nothing resets), or a failure the
/// image records before it resets. A restart of the station never ends it.
/// `E` is the start's refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationExitOf<E> {
    Stopped,
    /// Stopped and released, every owner back, but the network's stop
    /// participant was lost, so the network never confirmed its stop: a
    /// software error this exit records. The radio's stop was confirmed;
    /// nothing resets for it.
    StoppedNetworkLost,
    /// The lifecycle or the datapath actor under it ended terminally.
    Failed(LifecycleFailure),
    /// Stopped, but the Wi-Fi radio release did not complete: the stop is
    /// confirmed and nothing resets; a posted leave is never posted again.
    ReleaseFailed(ReleaseCause),
    /// The station never started: its refusal, and how the owners it
    /// returned ended.
    Unavailable(Unavailable<E>),
}

/// What the actor's task tells the station's task, in order, through the
/// one-slot installation mailbox.
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the chip's composition builds the other variants")
)]
pub(crate) enum News<C, K, R, E> {
    /// An installation, on first install and after every reinstall: its
    /// commands' issuer and the lifecycle's tokens.
    Installed(C, K),
    /// The first installation did not come up.
    Unavailable(Unavailable<E>),
    /// The port did not reinstall after the station's restart, and the
    /// removal receipt its cleanup freed, if one, which the receiver keeps.
    Reinstall {
        cause: ReinstallCause,
        returned: Option<R>,
    },
    /// The radio's release after the lifecycle retired the port, whether
    /// the last stop's network participant was lost, and the removal
    /// receipt the port's cleanup freed, if one, which the receiver keeps
    /// past the release, refused or not.
    Released {
        result: Result<(), ReleaseCause>,
        network_lost: bool,
        returned: Option<R>,
    },
}

/// The news of a restart: the stopped port's cleanup `cleaned` first (a
/// port it did not finish keeps its owners and does not reinstall), then
/// `reinstall`. A refusal of either carries the cleanup's receipt back
/// whole; after a reinstall that succeeded the receipt ends, as its owners
/// are the retired installation's.
pub(crate) fn reinstalled<C, K, R, E>(
    cleaned: Cleaned<R>,
    reinstall: impl FnOnce() -> Result<(C, K), ReinstallCause>,
) -> News<C, K, R, E> {
    let Cleaned { result, removed } = cleaned;
    if let Err(cause) = result {
        return News::Reinstall {
            cause: ReinstallCause::Cleanup(cause),
            returned: removed,
        };
    }
    match reinstall() {
        Ok((commands, tokens)) => {
            drop(removed);
            News::Installed(commands, tokens)
        }
        Err(cause) => News::Reinstall {
            cause,
            returned: removed,
        },
    }
}

/// The news of the retired port: its cleanup and its radio's release
/// ([`crate::release::release`]), whether the last stop's network
/// participant was lost, and the cleanup's receipt.
pub(crate) async fn retired<C, K, E, P: Stopped, F>(
    port: &mut P,
    network_lost: bool,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(P::Radio) -> F,
) -> News<C, K, P::Receipt, E>
where
    F: Future<Output = Leave<P::Radio, ReleaseCause>>,
{
    let (result, returned) = crate::release::release(port, timer, retry, leave, |_| {}).await;
    News::Released {
        result,
        network_lost,
        returned,
    }
}

/// The news the station waits for before a lifecycle: an installation, or
/// the exit it ends with and the receipt a refusal returned with it.
pub(crate) fn installed<C, K, R, E>(
    news: News<C, K, R, E>,
) -> Result<(C, K), (StationExitOf<E>, Option<R>)> {
    match news {
        News::Installed(commands, tokens) => Ok((commands, tokens)),
        News::Unavailable(error) => Err((StationExitOf::Unavailable(error), None)),
        // A refused reinstall: its receipt goes back with the refusal.
        News::Reinstall { cause, returned } => Err((
            StationExitOf::Failed(LifecycleFailure::Reinstall(cause)),
            returned,
        )),
        news @ News::Released { .. } => {
            let (_, returned) = ended(news);
            Err((StationExitOf::Failed(LifecycleFailure::Mailbox), returned))
        }
    }
}

/// The exit the actor task's last news gives, and the removal receipt a
/// refusal returned with it, for the runner's caller. Only a release that
/// succeeded ends its receipt, with the retired installation.
pub(crate) fn ended<C, K, R, E>(news: News<C, K, R, E>) -> (StationExitOf<E>, Option<R>) {
    match news {
        News::Released {
            result,
            network_lost,
            returned,
        } => {
            let returned = crate::release::returned(result.is_err(), returned);
            (released_exit(result, network_lost), returned)
        }
        News::Reinstall { returned, .. } => {
            (StationExitOf::Failed(LifecycleFailure::Mailbox), returned)
        }
        News::Installed(..) | News::Unavailable(_) => {
            (StationExitOf::Failed(LifecycleFailure::Mailbox), None)
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
