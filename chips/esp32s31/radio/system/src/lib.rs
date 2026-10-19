#![no_std]
#![forbid(unsafe_code)]

//! The ESP32-S31 radio system: one actor that owns the shared RF front-end.
//!
//! [`RadioSystem`] is one future, tied to no executor. It owns the PHY domain,
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
//! The actor is generic over its client `K`; this repository instantiates it
//! for Wi-Fi.

use core::{convert::Infallible, future::poll_fn};

use embassy_futures::select::{Either, select};
use oer_esp32s31_radio_client::{
    ActorEnd, Answer, Client, ClientInbox, DomainHealth, Doorbell, Failed, Failure, Joined,
    Leaving, Left, LpClock, LpSource, MemberMint, PhyPreparation, Request, RfEpoch, TrackedMember,
    wifi::Wifi,
};

/// A hardware transaction failed after it started: the PHY domain is
/// poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fault;

/// Why closing RF did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseFault {
    /// Nothing changed, or every issued transaction completed: RF stays open
    /// and the domain is healthy.
    Recoverable,
    /// The domain is poisoned.
    Poisoned,
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
    /// Enter `K` into the prepared domain, coexistence included.
    async fn enter(
        &mut self,
        join: K::Join,
        phy: PhyPreparation,
        lp: &LpClock<S>,
    ) -> Result<K::Joined, Failed<K::JoinReturned>>;
    /// Run `K`'s initial tracking and what its client contract adds to it.
    async fn track(&mut self, track: K::Track) -> Result<K::Tracked, Failed<K::TrackReturned>>;
    /// Exit `K` from the domain, coexistence included.
    async fn exit(&mut self, leave: K::Leave) -> (K::Left, Exit);
}

#[derive(Debug, Default)]
struct State {
    poisoned: bool,
    members: u32,
    tracked: u32,
    epoch: Option<RfEpoch>,
}

/// The radio system's actor for the client `K`.
pub struct RadioSystem<K: Client, S: LpSource, B, T> {
    backend: B,
    timer: T,
    lp: LpClock<S>,
    doorbell: &'static Doorbell,
    inbox: ClientInbox<K>,
    mint: MemberMint,
    state: State,
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
            mint,
            state: State::default(),
        }
    }
}

impl<K: Client, S: LpSource, B: ClientBackend<K, S>, T: oer_time::Timer> RadioSystem<K, S, B, T> {
    /// Serve the cells for the rest of the program.
    pub async fn run(mut self) -> Infallible {
        loop {
            while let Some((request, responder)) = self.inbox.take() {
                let answer = self.serve(request).await;
                responder.answer(answer);
            }
            let deadline = (!self.state.poisoned && self.state.tracked > 0)
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
                        self.tick().await;
                    }
                }
            }
        }
    }

    async fn tick(&mut self) {
        if self.backend.track_tick().await.is_err() {
            self.state.poisoned = true;
        }
    }

    async fn serve(&mut self, request: Request<K>) -> Answer<K> {
        match request {
            Request::Join(join) => Answer::Join(self.join(join).await),
            Request::Track(member, track) => {
                if self.state.poisoned {
                    return Answer::Track(Err(Failed {
                        returned: (member, K::track_returned(track)),
                        cause: Failure::Poisoned,
                    }));
                }
                Answer::Track(match self.backend.track(track).await {
                    Ok(payload) => {
                        self.state.tracked += 1;
                        Ok(TrackedMember {
                            member: self.mint.track(member),
                            payload,
                        })
                    }
                    Err(Failed { returned, cause }) => {
                        if cause == Failure::Poisoned {
                            self.state.poisoned = true;
                        }
                        Err(Failed {
                            returned: (member, returned),
                            cause,
                        })
                    }
                })
            }
            Request::Leave(member, leave) => Answer::Leave(self.leave(member, leave).await),
        }
    }

    async fn join(&mut self, join: K::Join) -> oer_esp32s31_radio_client::JoinAnswer<K> {
        if self.state.poisoned {
            return Err(Failed {
                returned: K::join_returned(join),
                cause: Failure::Poisoned,
            });
        }
        let phy = match self.backend.prepare().await {
            Ok(phy) => phy,
            Err(Fault) => {
                self.state.poisoned = true;
                return Err(Failed {
                    returned: K::join_returned(join),
                    cause: Failure::Poisoned,
                });
            }
        };
        if phy != PhyPreparation::AlreadyOpen {
            self.state.epoch = Some(self.mint.next_epoch(self.state.epoch));
        }
        let epoch = self
            .state
            .epoch
            .unwrap_or_else(|| self.mint.next_epoch(None));
        self.state.epoch = Some(epoch);
        match self.backend.enter(join, phy, &self.lp).await {
            Ok(payload) => {
                self.state.members += 1;
                Ok(Joined {
                    member: self.mint.untracked(),
                    epoch,
                    payload,
                })
            }
            Err(Failed { returned, cause }) => {
                if cause == Failure::Poisoned {
                    self.state.poisoned = true;
                } else if self.state.members == 0 {
                    self.close_rf().await;
                }
                Err(Failed {
                    returned,
                    cause: if self.state.poisoned {
                        Failure::Poisoned
                    } else {
                        cause
                    },
                })
            }
        }
    }

    async fn leave(&mut self, member: Leaving<K>, leave: K::Leave) -> Left<K> {
        if let Leaving::Tracked(_) = member {
            self.state.tracked = self.state.tracked.saturating_sub(1);
        }
        self.mint.retire(member);
        let (payload, exit) = self.backend.exit(leave).await;
        self.state.members = self.state.members.saturating_sub(1);
        if exit.health == DomainHealth::Poisoned {
            self.state.poisoned = true;
        } else if exit.last && !self.state.poisoned {
            self.close_rf().await;
        }
        Left {
            payload,
            domain: if self.state.poisoned {
                DomainHealth::Poisoned
            } else {
                DomainHealth::Healthy
            },
        }
    }

    async fn close_rf(&mut self) {
        match self.backend.close_rf(LastClientLeft(())).await {
            // RF stays open: the next join finds it open in this epoch.
            Ok(()) | Err(CloseFault::Recoverable) => {}
            Err(CloseFault::Poisoned) => self.state.poisoned = true,
        }
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
