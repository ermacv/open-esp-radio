#![no_std]
#![forbid(unsafe_code)]

//! The ESP32-S31 radio system: one actor that owns the shared RF front-end.
//!
//! [`RadioSystem`] owns the PHY domain; its run future borrows the actor and
//! is tied to no executor. The actor owns
//! the shared radio registers, common radio power, the modem clocks and
//! coexistence, through its [`ClientBackend`], and serves its clients'
//! cells (`oer-esp32s31-radio-client`) one request at a time. It takes no lock
//! and refuses nothing as busy: a request waits in its cell until the actor
//! takes it.
//!
//! - `join`: prepare the PHY domain (register it, or wake closed RF), mint a
//!   new [`RfEpoch`](oer_esp32s31_radio_client::RfEpoch) when RF opened,
//!   then let the backend enter the client, coexistence included in the
//!   order its hardware requires. A first client that fails without poisoning
//!   leaves RF closed again.
//! - `track`: the client's initial tracking; the member becomes `Tracked`.
//! - `leave`: the backend exits the client, coexistence included; when it
//!   was the last, the actor alone mints [`LastClientLeft`] and closes RF
//!   with it. A [`CloseFault::Recoverable`] close leaves RF open, and the
//!   next join finds it open in the same epoch.
//! - Between requests the actor runs the periodic tracking tick when it falls
//!   due, while a tracked member exists.
//!
//! A hardware failure poisons the domain: the request that saw it and every
//! later one are answered [`Failure::Poisoned`], handing back what they lent.
//! The actor keeps the first [`Poison`]: where it happened (a request, the
//! preparation, the RF close or the periodic tracking tick) and its step, so
//! a request refused by an earlier failed tick names that tick.
//! The primary client is `K`; Wi-Fi-only compositions contain no extra inbox.
//! The `bluetooth` feature adds its cell to the same run future and shared state.

use core::{convert::Infallible, future::poll_fn};

use embassy_futures::select::{Either, select};
use oer_esp32s31_radio_client::{
    ActorEnd, Answer, Client, ClientInbox, DomainHealth, Doorbell, Failed, Failure, Joined,
    Leaving, Left, LpClock, LpSource, MemberMint, PhyPreparation, Poison, PoisonCause,
    PoisonOrigin, Request, RfEpoch, TrackedMember, wifi::Wifi,
};

/// A hardware transaction failed after it started: the PHY domain is
/// poisoned by this cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fault(pub PoisonCause);

/// Why closing RF did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseFault {
    /// Nothing changed, or every issued transaction completed: RF stays open
    /// and the domain is healthy.
    Recoverable,
    /// The domain is poisoned by this cause.
    Poisoned(PoisonCause),
}

/// Proof that the last member left the PHY domain: only the actor mints it,
/// and only [`PhyDomain::close_rf`] consumes it.
#[derive(Debug)]
pub struct LastClientLeft(());

/// How a client's exit ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Exit {
    /// The client was the PHY domain's last member.
    pub last: bool,
    pub health: DomainHealth,
}

/// The client-neutral hardware of the PHY domain.
#[allow(
    async_fn_in_trait,
    reason = "the actor polls its backend on one executor; no Send bound is wanted"
)]
pub trait PhyDomain {
    /// Register the domain if it is not, wake RF if it is closed, or do
    /// nothing if RF is open.
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault>;
    /// Close RF after the last member left.
    async fn close_rf(&mut self, last: LastClientLeft) -> Result<(), CloseFault>;
    /// When the periodic tracking falls due, while a member is tracked.
    fn tracking_deadline(&self) -> Option<oer_time::Instant>;
    /// One periodic tracking tick.
    async fn track_tick(&mut self) -> Result<(), Fault>;
}

/// The hardware steps of one client `K` with the low-power clock `S`.
#[allow(
    async_fn_in_trait,
    reason = "the actor polls its backend on one executor; no Send bound is wanted"
)]
pub trait ClientBackend<K: Client, S: LpSource>: PhyDomain {
    /// Reject an unavailable installation before any shared RF work.
    fn admits(&self, _join: &K::Join) -> bool {
        true
    }
    /// Enter `K` into the prepared domain, coexistence included.
    async fn enter(
        &mut self,
        join: K::Join,
        phy: PhyPreparation,
        lp: &LpClock<S>,
    ) -> Result<K::Joined, Failed<K::JoinReturned>>;
    /// Run `K`'s initial tracking and what its client contract adds to it.
    async fn track(&mut self, track: K::Track) -> Result<K::Tracked, Failed<K::TrackReturned>>;
    /// Change a stopped tracked client's channel without changing membership.
    async fn retune(&mut self, retune: K::Retune) -> Result<K::Retuned, Failed<K::RetuneReturned>>;
    /// Exit `K` from the domain, coexistence included.
    async fn exit(&mut self, leave: K::Leave) -> (K::Left, Exit);
}

mod actor_core;
mod boundary;
use actor_core::{Action, ActorCore};

/// The radio system's actor for the client `K`.
pub struct RadioSystem<K: Client, S: LpSource, B, T> {
    backend: B,
    timer: T,
    lp: LpClock<S>,
    doorbell: &'static Doorbell,
    inbox: ClientInbox<K>,
    core: ActorCore,
}

impl<S: LpSource, B: ClientBackend<Wifi, S>, T: oer_time::Timer> RadioSystem<Wifi, S, B, T> {
    /// The Wi-Fi radio system over `backend`, with the claimed actor end.
    pub fn new(end: ActorEnd, backend: B, timer: T, lp: LpClock<S>) -> Self {
        let ActorEnd {
            doorbell,
            wifi,
            mint,
        } = end;
        Self {
            backend,
            timer,
            lp,
            doorbell,
            inbox: wifi,
            core: ActorCore::new(mint),
        }
    }
}

impl<K: Client, S: LpSource, B: ClientBackend<K, S>, T: oer_time::Timer> RadioSystem<K, S, B, T> {
    /// Serve the cells for the rest of the program, borrowing the actor's storage.
    pub async fn run(&mut self) -> Infallible {
        loop {
            while let Some((request, responder)) = self.inbox.take() {
                let answer = self.serve(request).await;
                responder.answer(answer);
            }
            let deadline = (self.core.tracking_active())
                .then(|| self.backend.tracking_deadline())
                .flatten();
            let doorbell = self.doorbell;
            let rung = poll_fn(|cx| doorbell.poll_rung(cx));
            match deadline {
                None => {
                    rung.await;
                }
                Some(deadline) => {
                    if let Either::Second(()) = select(rung, self.timer.wait_until(deadline)).await
                    {
                        boundary::shared(self.tick()).await;
                    }
                }
            }
        }
    }
}

impl<K: Client, S: LpSource, B: PhyDomain, T: oer_time::Timer> RadioSystem<K, S, B, T> {
    /// Access backend-local runtime ownership between separate borrows of run.
    /// The mutable borrow prevents the actor from running concurrently.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    #[cfg(any(feature = "bluetooth", test))]
    // Independent of the next composition future and attached controller.
    #[inline(always)]
    async fn wait(&self, deadline: Option<oer_time::Instant>) -> bool {
        let rung = poll_fn(|cx| self.doorbell.poll_rung(cx));
        let timer = boundary::Deadline::new(deadline.map(|at| self.timer.wait_until(at)));
        matches!(select(rung, timer).await, Either::Second(()))
    }

    async fn tick(&mut self) {
        if let Err(Fault(cause)) = self.backend.track_tick().await {
            self.core.poison(Poison {
                origin: PoisonOrigin::TrackingTick,
                cause,
            });
        }
    }

    async fn serve<Q: Client>(&mut self, request: Request<Q>) -> Answer<Q>
    where
        B: ClientBackend<Q, S>,
    {
        match request {
            Request::Join(join) => {
                if let Some(poison) = self.core.poisoned() {
                    return Answer::Join(Err(Failed {
                        returned: Q::join_returned(join),
                        cause: Failure::Poisoned(poison),
                    }));
                }
                if !self.backend.admits(&join) {
                    return Answer::Join(Err(Failed {
                        returned: Q::join_returned(join),
                        cause: Failure::Refused,
                    }));
                }
                let phy = match boundary::shared(self.backend.prepare()).await {
                    Ok(phy) => phy,
                    Err(Fault(cause)) => {
                        let poison = self.core.poison(Poison {
                            origin: PoisonOrigin::Prepare,
                            cause,
                        });
                        return Answer::Join(Err(Failed {
                            returned: Q::join_returned(join),
                            cause: Failure::Poisoned(poison),
                        }));
                    }
                };
                let epoch = self
                    .core
                    .prepared(matches!(phy, PhyPreparation::AlreadyOpen));
                Answer::Join(match self.backend.enter(join, phy, &self.lp).await {
                    Ok(payload) => {
                        self.core.joined();
                        Ok(Joined {
                            member: self.core.mint.untracked(),
                            epoch,
                            payload,
                        })
                    }
                    Err(Failed { returned, cause }) => {
                        if matches!(self.core.failed_enter(cause), Action::CloseRf) {
                            boundary::shared(self.close_rf()).await;
                        }
                        Err(Failed {
                            returned,
                            cause: self.core.failure(cause),
                        })
                    }
                })
            }
            Request::Track(member, track) => {
                if let Some(poison) = self.core.poisoned() {
                    return Answer::Track(Err(Failed {
                        returned: (member, Q::track_returned(track)),
                        cause: Failure::Poisoned(poison),
                    }));
                }
                Answer::Track(match self.backend.track(track).await {
                    Ok(payload) => {
                        self.core.tracked();
                        Ok(TrackedMember {
                            member: self.core.mint.track(member),
                            payload,
                        })
                    }
                    Err(Failed { returned, cause }) => {
                        self.core.failed(cause);
                        Err(Failed {
                            returned: (member, returned),
                            cause: self.core.failure(cause),
                        })
                    }
                })
            }
            Request::Retune(member, retune) => {
                if let Some(poison) = self.core.poisoned() {
                    return Answer::Retune(Err(Failed {
                        returned: (member, Q::retune_returned(retune)),
                        cause: Failure::Poisoned(poison),
                    }));
                }
                Answer::Retune(match self.backend.retune(retune).await {
                    Ok(payload) => Ok(oer_esp32s31_radio_client::RetunedMember { member, payload }),
                    Err(Failed { returned, cause }) => {
                        self.core.failed(cause);
                        Err(Failed {
                            returned: (member, returned),
                            cause: self.core.failure(cause),
                        })
                    }
                })
            }
            Request::Leave(member, leave) => {
                self.core.leaving(matches!(member, Leaving::Tracked(_)));
                self.core.mint.retire(member);
                let (payload, exit) = self.backend.exit(leave).await;
                if matches!(self.core.exited(exit), Action::CloseRf) {
                    boundary::shared(self.close_rf()).await;
                }
                Answer::Leave(Left {
                    payload,
                    domain: self.core.health(),
                })
            }
        }
    }

    async fn close_rf(&mut self) {
        let result = self.backend.close_rf(LastClientLeft(())).await;
        self.core.closed(result);
    }
}

#[cfg(any(target_arch = "riscv32", test))]
mod channel;
#[cfg(target_arch = "riscv32")]
mod esp32s31;
#[cfg(any(target_arch = "riscv32", test))]
mod mac_sources;
#[cfg(target_arch = "riscv32")]
pub use esp32s31::Esp32s31Radio;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_clocks;

#[cfg(feature = "bluetooth")]
mod bluetooth;

#[cfg(feature = "bluetooth")]
pub use bluetooth::{BluetoothActor, BluetoothBackend};
#[cfg(all(target_arch = "riscv32", feature = "bluetooth"))]
pub use esp32s31::BluetoothAttachment;
