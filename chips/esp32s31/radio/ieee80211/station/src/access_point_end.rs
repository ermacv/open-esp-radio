//! How the access point's run ends: its exit, and the retire step that
//! stops its radio unless the port confirmed the stop, releases it and
//! returns a refused release's cleanup receipt to the runner's caller.
//! Generic over the ended runner ([`Retiring`]): the chip's is the access
//! point's runner over the S31 port; a host test names its own.

use core::{cell::Cell, future::Future};

use oer_portable_time::{Duration, Timer};
use oer_protocols_ieee80211_contracts_port::{ReceiveAccounting, StopUnconfirmed};
use oer_protocols_ieee80211_runtime::access_point::ApUnsupported;

use crate::release::{Leave, Stopped};

/// Why the access point's run ended.
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the chip's composition builds the other variants")
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApEnded {
    /// The application stopped it.
    Stopped,
    /// Its radio did not start.
    StartRefused(
        oer_protocols_ieee80211_contracts_port::StartRefused<crate::failure::BackendFault>,
    ),
    /// The port reported its named fault.
    Faulted(crate::failure::BackendFault),
    /// The runner's own contract broke.
    Contract(oer_protocols_ieee80211_runtime::access_point::ApContract),
    /// The port refused one of its commands.
    Refused(oer_protocols_ieee80211_contracts_port::AccessPointRefusal),
    /// The port refused a received unit's recycle for its count; the unit
    /// stayed with the runner and ended with it after the release.
    Unrecycled(oer_protocols_ieee80211_contracts_port::ReceiveAccounting),
    /// The port's capabilities do not serve the access point; the run
    /// never asked the radio to start.
    Unsupported(ApUnsupported),
}

/// How the access point ended, or how far its end came; `E` is the
/// start's refusal.
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the chip's composition builds the other variants")
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApExitOf<E> {
    /// It ended for its cause; its radio stopped, every owner came back
    /// and the radio was released.
    Ended(ApEnded),
    /// It ended for its cause and its radio's stop is not confirmed yet, as
    /// the port observed it last: the runner keeps every owner the hardware
    /// may still use and watches the stop on. Published while it does; the
    /// run never returns it.
    StopUnconfirmed {
        /// Why it ended.
        ended: ApEnded,
        /// What the port observed of the stop.
        observed: oer_protocols_ieee80211_contracts_port::StopUnconfirmed,
    },
    /// It ended for its cause, its radio's stop was confirmed, and the
    /// radio's release refused: published while the MAC refuses it as
    /// still active and the release tries again; returned for a refusal
    /// the release does not repeat (a leave never answered, a poisoned
    /// domain, a port with no stopped radio).
    ReleaseRefused {
        /// Why it ended.
        ended: ApEnded,
        /// Why the release refused.
        cause: crate::failure::ReleaseCause,
    },
    /// It ended for its cause, and the stop that followed found a received
    /// unit whose recycle the port refused for its count: the unit stayed
    /// with the runner and ended with it after the release.
    Unrecycled {
        /// Why it ended.
        ended: ApEnded,
        /// The count that refused the unit.
        fault: oer_protocols_ieee80211_contracts_port::ReceiveAccounting,
        /// How the release ended.
        released: Result<(), crate::failure::ReleaseCause>,
    },
    /// It never served: its refusal before the runner existed, and how the
    /// owners that refusal returned ended.
    Unavailable(crate::failure::Unavailable<E>),
}

/// What a run's end left of the radio's stop.
pub(crate) enum Stop {
    /// The port confirmed it.
    Confirmed,
    /// The port did not confirm it yet, as it observed.
    Unconfirmed(StopUnconfirmed),
    /// The run ended before it.
    Owed,
}

/// An access point's runner whose run ended: the chip's runner over the S31
/// port, a runner of the test on the host.
pub(crate) trait Retiring {
    /// Its port, whose stopped radio the release takes.
    type Port: Stopped;
    /// A received unit the port refused, which stays with the runner.
    type Unrecycled;
    /// Watch the radio's stop until the port confirms it, each observation
    /// going to `unconfirmed`; a unit whose recycle the port refused is the
    /// error.
    fn finish_stop(
        &mut self,
        unconfirmed: impl FnMut(StopUnconfirmed),
    ) -> impl Future<Output = Result<(), ReceiveAccounting>>;
    /// The unit the port refused, once.
    fn take_unrecycled(&mut self) -> Self::Unrecycled;
    /// Its port.
    fn port_mut(&mut self) -> &mut Self::Port;
}

/// Stop the radio of an ended run unless the port confirmed its stop, then
/// release it ([`crate::release::release`] through `leave`, every `retry`
/// after a refusal that posted nothing). Until the port confirms the stop,
/// the runner keeps every owner and watches the stop on, each observation
/// published in `published` as [`ApExitOf::StopUnconfirmed`]; until the
/// release ends, a release the MAC refused as still active is published as
/// [`ApExitOf::ReleaseRefused`] and tried again. The owners end with the
/// runner once the stop is confirmed and the release ended; a refused
/// release's cleanup receipt goes back to the runner's caller with it.
pub(crate) async fn retire<R: Retiring, E: Copy, F>(
    published: &Cell<Option<ApExitOf<E>>>,
    mut runner: R,
    ended: ApEnded,
    stop: Stop,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(<R::Port as Stopped>::Radio) -> F,
) -> (ApExitOf<E>, Option<<R::Port as Stopped>::Receipt>)
where
    F: Future<Output = Leave<<R::Port as Stopped>::Radio, crate::failure::ReleaseCause>>,
{
    let unconfirmed = |observed| published.set(Some(ApExitOf::StopUnconfirmed { ended, observed }));
    let finished = match stop {
        Stop::Confirmed => Ok(()),
        Stop::Unconfirmed(observed) => {
            unconfirmed(observed);
            runner.finish_stop(unconfirmed).await
        }
        Stop::Owed => runner.finish_stop(unconfirmed).await,
    };
    // A unit the port refused stays here until the runner ends, after the
    // release.
    let unrecycled = runner.take_unrecycled();
    let refused = |cause| published.set(Some(ApExitOf::ReleaseRefused { ended, cause }));
    // The cleanup's removal receipt: a refused release returns it to the
    // runner's caller with the refusal; after a release that succeeded it
    // ends with the retired installation.
    let (released, returned) =
        crate::release::release(runner.port_mut(), timer, retry, leave, refused).await;
    drop(unrecycled);
    let returned = crate::release::returned(released.is_err(), returned);
    let exit = match (ended, finished, released) {
        // A refusal the stop found after another cause.
        (ended, Err(fault), released) if !matches!(ended, ApEnded::Unrecycled(_)) => {
            ApExitOf::Unrecycled {
                ended,
                fault,
                released,
            }
        }
        (ended, _, Ok(())) => ApExitOf::Ended(ended),
        (ended, _, Err(cause)) => ApExitOf::ReleaseRefused { ended, cause },
    };
    (exit, returned)
}

#[cfg(test)]
mod tests;
