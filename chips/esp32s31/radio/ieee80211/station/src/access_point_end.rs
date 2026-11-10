//! How the access point's run ends: its exit, and the retire step that
//! stops its radio unless the port confirmed the stop, releases it and
//! returns a refused release's cleanup receipt to the runner's caller.
//! Generic over the ended runner ([`Retiring`]): the chip's is the access
//! point's runner over the S31 port; a host test names its own.

use core::{cell::Cell, future::Future};

use oer_portable_time::{Duration, Timer};
use oer_protocols_ieee80211_contracts_port::{ReceiveAccounting, StopUnconfirmed};
use oer_protocols_ieee80211_runtime::access_point::{ApUnsupported, StopWatch};

use crate::release::{Leave, Stopped};
use crate::unfinished::{Phase, UNKEPT_ITEMS, Unfinished};

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
    /// stays with the runner: it ends with it after a completed retire, and
    /// comes back inside it otherwise.
    Unrecycled(oer_protocols_ieee80211_contracts_port::ReceiveAccounting),
    /// The port's capabilities do not serve the access point; the run
    /// never asked the radio to start.
    Unsupported(ApUnsupported),
    /// A port call returned an item it could neither give the runner nor
    /// keep; the run made no port call after it, and the retire took the
    /// item before it watched the stop on.
    Unkept,
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
    /// unit whose recycle the port refused for its count: the unit stays
    /// with the runner, ended with it after a released radio and returned
    /// inside it after a refused release.
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
    /// It ended for its cause, and port calls of the run returned items
    /// they could neither give the runner nor keep: no release ran, and the
    /// installation came back unfinished with them
    /// ([`crate::unfinished::Unfinished`]).
    Unkept {
        /// Why it ended.
        ended: ApEnded,
        /// How far the radio's stop came.
        stop: UnkeptStop,
    },
}

/// How far the radio's stop came when an access point's retire ended on
/// items a port call could not keep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnkeptStop {
    /// Not confirmed: the retire's home had no room for a further item,
    /// which the runner keeps; the hardware may still use the owners.
    Unconfirmed,
    /// Confirmed.
    Confirmed,
    /// Confirmed, and the stop found a received unit whose recycle the
    /// port refused for this count; the unit is in the runner.
    Unrecycled(ReceiveAccounting),
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
    /// An item a port call of the run could not keep.
    type Unkept;
    /// Watch the radio's stop until the port confirms it, each observation
    /// going to `unconfirmed`, or until the runner holds an item a port
    /// call could not keep ([`StopWatch::Unkept`]); a unit whose recycle
    /// the port refused is the error.
    fn finish_stop(
        &mut self,
        unconfirmed: impl FnMut(StopUnconfirmed),
    ) -> impl Future<Output = Result<StopWatch, ReceiveAccounting>>;
    /// Its port.
    fn port_mut(&mut self) -> &mut Self::Port;
    /// The item a port call of the run could not keep, once.
    fn take_unkept(&mut self) -> Option<Self::Unkept>;
}

/// The items a port call of the run could not keep, which the retire holds
/// while it goes on with the stop: its home's places, filled in order.
pub type UnkeptItems<I> = [Option<I>; UNKEPT_ITEMS];

/// Move the item the runner holds, if one, to the home's first free place;
/// with no free place the runner keeps it.
fn take_into<R: Retiring>(home: &mut UnkeptItems<R::Unkept>, runner: &mut R) {
    if let Some(place) = home.iter_mut().find(|place| place.is_none()) {
        *place = runner.take_unkept();
    }
}

/// An access point's installation that did not retire: its runner, the
/// items its port calls could not keep, the cleanup's receipt, its exit as
/// the cause, and its phase.
pub type UnfinishedAccessPoint<R, I, Receipt, E> =
    Unfinished<R, UnkeptItems<I>, (), Receipt, ApExitOf<E>>;

/// How the retire of an ended access point ended.
#[must_use = "an unfinished retire holds the runner, its port and their owners"]
pub(crate) enum Retired<R: Retiring, E> {
    /// The radio's stop was confirmed and the radio released: every owner
    /// ended with the runner.
    Completed(ApExitOf<E>),
    /// The installation did not retire: everything it held.
    Unfinished(UnfinishedAccessPoint<R, R::Unkept, <R::Port as Stopped>::Receipt, E>),
}

/// Stop the radio of an ended run unless the port confirmed its stop, then
/// release it ([`crate::release::release`] through `leave`, every `retry`
/// after a refusal that posted nothing).
///
/// An item a port call of the run could not keep is taken from the runner
/// before the stop is watched, and after each watch that returned with
/// one, into a free place of the retire's home ([`UNKEPT_ITEMS`]), so the
/// watch goes on serving the port; a further item that finds the home
/// full stays with the runner and ends the retire unfinished
/// ([`Phase::Stopping`]). Until the port confirms the
/// stop, the runner keeps every owner and watches the stop on, each
/// observation published in `published` as [`ApExitOf::StopUnconfirmed`].
/// With the stop confirmed and items out, no release runs
/// ([`Phase::Stopped`]); otherwise a release the MAC refused as still
/// active is published as [`ApExitOf::ReleaseRefused`] and tried again,
/// and a refusal it does not repeat ends the retire unfinished
/// ([`Phase::Releasing`]). Only a release that succeeded completes the
/// retire: the owners, the cleanup's receipt among them, end with the
/// runner. Every unfinished end returns the runner, its port, the items
/// and the receipt whole.
pub(crate) async fn retire<R: Retiring, E: Copy, F>(
    published: &Cell<Option<ApExitOf<E>>>,
    mut runner: R,
    ended: ApEnded,
    stop: Stop,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(<R::Port as Stopped>::Radio) -> F,
) -> Retired<R, E>
where
    F: Future<Output = Leave<<R::Port as Stopped>::Radio, crate::failure::ReleaseCause>>,
{
    let unconfirmed = |observed| published.set(Some(ApExitOf::StopUnconfirmed { ended, observed }));
    let mut unkept: UnkeptItems<R::Unkept> = core::array::from_fn(|_| None);
    let finished = match stop {
        Stop::Confirmed => Ok(()),
        stop => {
            if let Stop::Unconfirmed(observed) = stop {
                unconfirmed(observed);
            }
            loop {
                // An item the runner holds goes to a free place of the
                // home before the watch, which makes no port call while
                // the runner holds one.
                take_into(&mut unkept, &mut runner);
                match runner.finish_stop(unconfirmed).await {
                    Ok(StopWatch::Confirmed) => break Ok(()),
                    // The next turn of the loop takes the item, while the
                    // home has room; a full home leaves it with the
                    // runner, and the retire ends there.
                    Ok(StopWatch::Unkept) if unkept.iter().all(Option::is_some) => {
                        return Retired::Unfinished(Unfinished {
                            owner: runner,
                            unkept,
                            unaccepted: (),
                            receipt: None,
                            cause: ApExitOf::Unkept {
                                ended,
                                stop: UnkeptStop::Unconfirmed,
                            },
                            phase: Phase::Stopping,
                        });
                    }
                    Ok(StopWatch::Unkept) => {}
                    Err(fault) => break Err(fault),
                }
            }
        }
    };
    // An item the run left with a stop confirmed before the retire; with
    // the home full it stays with the runner.
    take_into(&mut unkept, &mut runner);
    // Items a port call could not keep: no release follows them, and they
    // go back with the runner.
    if unkept.iter().any(Option::is_some) {
        let stop = match finished {
            Ok(()) => UnkeptStop::Confirmed,
            Err(fault) => UnkeptStop::Unrecycled(fault),
        };
        return Retired::Unfinished(Unfinished {
            owner: runner,
            unkept,
            unaccepted: (),
            receipt: None,
            cause: ApExitOf::Unkept { ended, stop },
            phase: Phase::Stopped,
        });
    }
    let refused = |cause| published.set(Some(ApExitOf::ReleaseRefused { ended, cause }));
    let (released, returned) =
        crate::release::release(runner.port_mut(), timer, retry, leave, refused).await;
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
    match released {
        // The receipt ends with the retired installation's other owners.
        Ok(()) => {
            drop(returned);
            drop(runner);
            Retired::Completed(exit)
        }
        Err(_) => Retired::Unfinished(Unfinished {
            owner: runner,
            unkept,
            unaccepted: (),
            receipt: returned,
            cause: exit,
            phase: Phase::Releasing,
        }),
    }
}

#[cfg(test)]
mod tests;
