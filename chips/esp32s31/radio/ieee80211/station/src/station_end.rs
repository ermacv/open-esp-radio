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
pub(crate) enum News<C, K, E, B> {
    /// An installation, on first install and after every reinstall: its
    /// commands' issuer, the lifecycle's tokens and which installation it
    /// is.
    Installed(C, K, InstallationOf<B>),
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

/// One installation of a station's port: the slot identity `B` it was
/// minted for, a type, and its generation, which the identity's registry
/// hands out once ([`oer_protocols_radio_port::Installations`]). Two
/// installations of one identity never share a generation, a reinstall in
/// the same places included.
pub struct InstallationOf<B> {
    generation: u32,
    identity: core::marker::PhantomData<fn(B) -> B>,
}

impl<B> InstallationOf<B> {
    /// The installation of identity `B` the registry minted as
    /// `generation`.
    pub(crate) const fn minted(generation: u32) -> Self {
        Self {
            generation,
            identity: core::marker::PhantomData,
        }
    }

    /// Its generation.
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl<B> Clone for InstallationOf<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for InstallationOf<B> {}

impl<B> PartialEq for InstallationOf<B> {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl<B> Eq for InstallationOf<B> {}

impl<B> core::fmt::Debug for InstallationOf<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Installation")
            .field(&self.generation)
            .finish()
    }
}

/// The proof that one station installation's datapath half retired: its
/// radio's release completed and every owner of that half ended. Only
/// [`retired`] makes it, once per completed release, for the installation
/// it retired; it lets that installation's lifecycle half, and no other,
/// finish without a release of its own ([`UnfinishedLifecycleOf::finish`]).
/// A proof of another identity `B` does not type-check there; one of
/// another generation of the same identity comes back unused.
#[must_use = "the proof finishes the lifecycle's half of the retired installation"]
pub struct DatapathRetired<B> {
    installation: InstallationOf<B>,
}

impl<B> DatapathRetired<B> {
    /// The installation that retired.
    pub const fn installation(&self) -> InstallationOf<B> {
        self.installation
    }
}

impl<B> core::fmt::Debug for DatapathRetired<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("DatapathRetired")
            .field(&self.installation)
            .finish()
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
/// (`abandoned`). The abandon is looked for before each poll of the run,
/// its first included: once it is queued, no run makes progress, so after
/// it the actor's task posts at most the one installation's news of a
/// reinstall already past its run. An abandon drops the run first, so the
/// actor produces nothing more into the queues between them, and only then
/// tells the lifecycle (`mailbox`, [`News::Abandoned`]), which waits for it
/// before its own ends go out ([`UnfinishedLifecycleOf::abandoned`]).
///
/// # Errors
///
/// The lifecycle's failure, once the run stopped and the news was posted.
pub(crate) async fn run_until_abandoned<F: Future, C, K, E, B>(
    run: F,
    abandoned: &mut oer_portable_memory::WakingSpscReceiver<'_, LifecycleFailure, 1>,
    mailbox: &mut oer_portable_memory::WakingSpscSender<'_, News<C, K, E, B>, 1>,
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
        // The first future is polled first.
        match embassy_futures::select::select(abandon, run).await {
            embassy_futures::select::Either::First(failure) => failure,
            embassy_futures::select::Either::Second(end) => return Ok(end),
        }
    };
    // The run is dropped here: the news follows its last poll.
    mailbox.ready().await.send(News::Abandoned);
    Err(failure)
}

/// How the actor's task ended after the lifecycle stopped waiting for it,
/// as its late news told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LateEnd<E> {
    /// Its last news: a release past the stop's drain, a fault, a refused
    /// reinstall.
    Ended(StationExitOf<E>),
    /// Its run stopped on the lifecycle's abandon: its half is the
    /// datapath's unfinished installation.
    Abandoned,
}

/// What a lifecycle half has read of the actor task's end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LateRead<E> {
    /// The actor's task told how it ended: the confirmation, kept once
    /// read.
    Ended(LateEnd<E>),
    /// It told nothing that ends it yet: its mailbox is empty.
    Running,
    /// Every place for installations' news is full ([`INSTALLED`]): more
    /// came than the actor's task posts before its end, a broken bound.
    /// Nothing is confirmed and nothing more is read; once the
    /// application took a kept news
    /// ([`UnfinishedLifecycleOf::take_installed`]) the half reads on
    /// ([`UnfinishedLifecycleOf::confirm_end`]), the next news still in
    /// the mailbox with its owners.
    Full,
}

/// Every place for installations' news of a lifecycle half is full: the
/// end is not read, and is read only once a kept news was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeptFull;

/// The station lifecycle's half of an installation that did not retire,
/// whole: its retired lifecycle `R`, its ends `Q` of the queues to the
/// actor with every owner in transit in them, in order, which
/// installation of the identity `B` it ran on, and the actor task's news
/// that came after the lifecycle stopped waiting for it, the owners an
/// installation's news carried kept.
#[must_use = "the lifecycle's half holds the station's owners of the installation"]
pub struct UnfinishedLifecycleOf<'a, R, Q, C, K, E, B> {
    retired: R,
    queues: Q,
    /// The installation the actor's task runs on now: the one the
    /// lifecycle ran on, then each one a news of an installation named, in
    /// order. Taking a kept news does not change it.
    current: InstallationOf<B>,
    mailbox: oer_portable_memory::WakingSpscReceiver<'a, News<C, K, E, B>, 1>,
    /// The news that ended the actor's task, once read.
    late: Option<LateEnd<E>>,
    /// The commands, tokens and installation a news of an installation
    /// carried, kept in their order ([`INSTALLED`]).
    installed: [Option<Installed<C, K, B>>; INSTALLED],
}

/// The places for installations' news a lifecycle half keeps. At most two
/// come before the actor task's end after an abandon: one already in the
/// mailbox when the lifecycle abandons, and one of a reinstall past its run
/// ([`run_until_abandoned`] looks for the abandon before each poll of the
/// run). The third keeps one past that bound, which the actor's task does
/// not post, so the end is read with a place still free.
pub const INSTALLED: usize = 3;

/// How a read of the actor task's news went.
enum Read {
    /// One news was read.
    Taken,
    /// The mailbox is empty.
    Empty,
    /// Every place for installations' news is full: nothing was read.
    Full,
}

/// What a finished lifecycle half hands back: how the actor's task ended
/// after the lifecycle stopped waiting, the lifecycle's ends of the queues
/// with every owner still in transit in them, and the commands, tokens
/// and installation a news of an installation carried.
#[must_use = "the finished half returns the owners still in transit"]
pub struct FinishedLifecycleOf<Q, C, K, E, B> {
    /// How the actor's task ended, once it told.
    pub late: Option<LateEnd<E>>,
    /// The lifecycle's ends of the queues to the actor.
    pub queues: Q,
    /// What a news of an installation carried, in order.
    pub installed: [Option<Installed<C, K, B>>; INSTALLED],
}

/// What a news of an installation carried: its commands, its tokens and
/// which installation it is.
pub type Installed<C, K, B> = (C, K, InstallationOf<B>);

/// How a lifecycle half `H`'s finish ended: the finished half, or the half
/// and the proof it refused, unchanged.
pub type Finished<H, Q, C, K, E, B> =
    Result<FinishedLifecycleOf<Q, C, K, E, B>, RefusedProof<H, B>>;

/// Why a lifecycle half refused a proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProofRefusal {
    /// The proof names another installation than the one the actor's task
    /// runs on now ([`UnfinishedLifecycleOf::installation`]).
    Foreign,
    /// Every place for installations' news is full and the actor task's
    /// end is not read: the half reads on once the application took a
    /// kept news ([`LateRead::Full`]).
    Unread,
}

/// A proof refused by a lifecycle half: both come back unchanged, nothing
/// finished or consumed.
#[must_use = "the refused finish returns the half and the proof whole"]
pub struct RefusedProof<H, B> {
    /// The half, unchanged.
    pub half: H,
    /// The proof, unchanged.
    pub proof: DatapathRetired<B>,
    /// Why it was refused.
    pub cause: ProofRefusal,
}

impl<'a, R, Q, C, K, E: Copy, B> UnfinishedLifecycleOf<'a, R, Q, C, K, E, B> {
    /// The half of `retired`, which ran on `installation`, with the
    /// lifecycle's `queues` and `mailbox`, and `late`, the actor task's end
    /// when the lifecycle already read it.
    pub(crate) const fn new(
        retired: R,
        queues: Q,
        installation: InstallationOf<B>,
        mailbox: oer_portable_memory::WakingSpscReceiver<'a, News<C, K, E, B>, 1>,
        late: Option<LateEnd<E>>,
    ) -> Self {
        Self {
            retired,
            queues,
            current: installation,
            mailbox,
            late,
            installed: [None, None, None],
        }
    }

    /// The half of a lifecycle that ended on `failure` without retiring the
    /// actor: the actor's task hears it once (`abandon`), and the half is
    /// returned only once the actor's task told that it ended (its run
    /// stopped: [`News::Abandoned`], or the news it ended on first). A
    /// news of an installation before it does not end the wait: its
    /// commands and tokens are kept in the half, and the wait goes on;
    /// nothing but the end of the actor's task ends it. Only then do the
    /// lifecycle's ends of the queues go out, so no producer runs while
    /// they are collected.
    ///
    /// With every place for installations' news full ([`INSTALLED`]) the
    /// bound broke: the wait stops unconfirmed ([`LateRead::Full`]), the
    /// next news left in the mailbox, and the application continues it
    /// ([`Self::confirm_end`]) once it took a kept news. A full store never
    /// stands for the end.
    pub(crate) async fn abandoned(
        mut self,
        failure: LifecycleFailure,
        abandon: &mut oer_portable_memory::WakingSpscSender<'_, LifecycleFailure, 1>,
    ) -> Self {
        abandon.ready().await.send(failure);
        // Confirmed, or every place full: the half tells which
        // ([`Self::late_end`]).
        let _read = self.confirm_end().await;
        self
    }

    /// Wait until the actor's task tells how it ended, keeping each news of
    /// an installation it reads on the way: the managed continuation of an
    /// abandon whose wait stopped on full places, once the application
    /// took a kept news ([`Self::take_installed`]). An end already read
    /// returns at once.
    ///
    /// # Errors
    ///
    /// [`KeptFull`]: every place for installations' news is full and the
    /// end is not read; nothing more was read.
    pub async fn confirm_end(&mut self) -> Result<LateEnd<E>, KeptFull> {
        loop {
            if let Some(late) = self.late {
                return Ok(late);
            }
            match self.read_one() {
                Read::Taken => {}
                Read::Empty => self.mailbox.ready().await,
                Read::Full => return Err(KeptFull),
            }
        }
    }

    /// Read the next news of the actor's task, when there is room for it:
    /// a news of an installation is kept; any other one ends the task.
    fn read_one(&mut self) -> Read {
        let Some(place) = self.installed.iter_mut().find(|place| place.is_none()) else {
            return Read::Full;
        };
        let Some(news) = self.mailbox.try_receive() else {
            return Read::Empty;
        };
        self.late = match news {
            News::Installed(commands, tokens, installation) => {
                // The actor's task runs on that installation now.
                self.current = installation;
                *place = Some((commands, tokens, installation));
                return Read::Taken;
            }
            News::Abandoned => Some(LateEnd::Abandoned),
            News::Reinstall(cause) => Some(LateEnd::Ended(StationExitOf::Failed(
                LifecycleFailure::Reinstall(cause),
            ))),
            news => Some(LateEnd::Ended(ended(news))),
        };
        Read::Taken
    }

    /// The retired lifecycle.
    pub(crate) fn retired_mut(&mut self) -> &mut R {
        &mut self.retired
    }

    /// The installation the actor's task runs on now, as far as its news
    /// was read: the one the lifecycle ran on, or the last one a news of an
    /// installation named. Only its proof finishes the half.
    pub const fn installation(&self) -> InstallationOf<B> {
        self.current
    }

    /// The lifecycle's ends of the queues to the actor: the owners in
    /// transit in them, each once, in order.
    pub fn queues(&mut self) -> &mut Q {
        &mut self.queues
    }

    /// How far the actor task's end is read, its news read on now without
    /// waiting: the end, kept once read; nothing that ends it yet; or every
    /// place for installations' news full. A news of an installation is
    /// kept ([`Self::take_installed`]) and the next one read.
    pub fn late_end(&mut self) -> LateRead<E> {
        loop {
            if let Some(late) = self.late {
                return LateRead::Ended(late);
            }
            match self.read_one() {
                Read::Taken => {}
                Read::Empty => return LateRead::Running,
                Read::Full => return LateRead::Full,
            }
        }
    }

    /// The commands, tokens and installation a news of an installation
    /// carried, each once, in order. The installation the actor's task
    /// runs on ([`Self::installation`]) stays.
    pub fn take_installed(&mut self) -> Option<Installed<C, K, B>> {
        self.installed.iter_mut().find_map(Option::take)
    }

    /// Finish the half of an installation whose datapath half retired: the
    /// retired lifecycle's owners end with the retired installation, as
    /// after a completed retire, and no release runs again (the datapath
    /// half's release was the installation's one). The actual end of the
    /// actor's task ([`Self::late_end`]), beside the exit the lifecycle's
    /// task returned, and every owner still in transit come back.
    ///
    /// # Errors
    ///
    /// The half and the proof come back unchanged, nothing ended, when the
    /// places for installations' news are full and the end is unread
    /// ([`ProofRefusal::Unread`]), or when the proof names another
    /// installation than the one the actor's task runs on now
    /// ([`ProofRefusal::Foreign`]: a kept earlier or later one included).
    #[allow(
        clippy::result_large_err,
        reason = "a refused finish returns the half and the proof whole, by value"
    )]
    pub fn finish(mut self, retired: DatapathRetired<B>) -> Finished<Self, Q, C, K, E, B> {
        // Read on first: a news of the proof's installation moves the
        // current one to it.
        let read = self.late_end();
        let cause = if matches!(read, LateRead::Full) {
            Some(ProofRefusal::Unread)
        } else if retired.installation != self.current {
            Some(ProofRefusal::Foreign)
        } else {
            None
        };
        if let Some(cause) = cause {
            return Err(RefusedProof {
                half: self,
                proof: retired,
                cause,
            });
        }
        let DatapathRetired { installation: _ } = retired;
        let late = self.late;
        let Self {
            retired: _retired,
            queues,
            current: _,
            mailbox: _mailbox,
            late: _,
            installed,
        } = self;
        Ok(FinishedLifecycleOf {
            late,
            queues,
            installed,
        })
    }
}
/// One side's ends of the queues between the actor and the station's
/// lifecycle: closing what this side receives, and whether an owner is
/// still in transit to it.
pub(crate) trait InTransit {
    /// Close the reception of every queue this side receives from: once it
    /// completed, no send is in progress into them and none publishes
    /// after it; a later send or permit gets its owner back whole.
    async fn close(&mut self);
    /// Whether a queue this side receives from holds an owner.
    fn holds(&self) -> bool;
}

/// `queues` of a completed retire, once their producers stopped: their
/// reception is closed first, so the check that follows is final. `None`
/// when no owner is in transit in them to this side, so they end empty
/// with the retired installation; otherwise the queues, whole, with every
/// owner in them, each once, in order. Either way a send into them later
/// returns its owner whole.
pub(crate) async fn in_transit<Q: InTransit>(mut queues: Q) -> Option<Q> {
    queues.close().await;
    queues.holds().then_some(queues)
}

/// The lifecycle's side: what the actor sent and the lifecycle did not
/// take.
impl<'id, D, const TO: usize, const FROM: usize, const RETURNS: usize> InTransit
    for oer_protocols_ieee80211_runtime::lifecycle::ActorEnds<'_, 'id, D, TO, FROM, RETURNS>
where
    D: oer_protocols_ieee80211_runtime::datapath::DatapathPort<'id>,
{
    async fn close(&mut self) {
        self.from_actor.close().await;
        self.returns.close().await;
        self.groups.close().await;
        self.link_return.close().await;
    }

    fn holds(&self) -> bool {
        !(self.from_actor.is_empty()
            && self.returns.is_empty()
            && self.groups.is_empty()
            && self.link_return.is_empty())
    }
}

/// The actor's side: what the lifecycle sent and the actor did not take.
impl<'id, D, const TO: usize, const FROM: usize, const RETURNS: usize> InTransit
    for oer_protocols_ieee80211_runtime::datapath::LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>
where
    D: oer_protocols_ieee80211_runtime::datapath::DatapathPort<'id>,
{
    async fn close(&mut self) {
        self.to_actor.close().await;
        self.parcels.close().await;
        self.link.close().await;
    }

    fn holds(&self) -> bool {
        !(self.to_actor.is_empty() && self.parcels.is_empty() && self.link.is_empty())
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
pub(crate) fn reinstalled<A: Installation, C, K, E, B>(
    mut actor: A,
    reinstall: impl FnOnce(&mut A) -> Result<(C, K, InstallationOf<B>), ReinstallCause>,
) -> Result<(A, News<C, K, E, B>), (News<C, K, E, B>, UnfinishedOf<A>)> {
    let Cleaned { result, removed } = actor.port_mut().clean();
    let refused = match result {
        Err(cause) => ReinstallCause::Cleanup(cause),
        Ok(()) => match reinstall(&mut actor) {
            Ok((commands, tokens, installation)) => {
                drop(removed);
                return Ok((actor, News::Installed(commands, tokens, installation)));
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
/// cleanup's receipt end with it, and the proof that `installation`
/// retired is returned ([`DatapathRetired`]). A refusal leaves it
/// unfinished, whole, in [`Phase::Releasing`].
pub(crate) async fn retired<A: Installation, C, K, E, B, F>(
    mut actor: A,
    installation: InstallationOf<B>,
    network_lost: bool,
    timer: &impl Timer,
    retry: Duration,
    leave: impl FnMut(<A::Port as Stopped>::Radio) -> F,
) -> (
    News<C, K, E, B>,
    Result<DatapathRetired<B>, UnfinishedOf<A>>,
)
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
            Ok(DatapathRetired { installation })
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
#[allow(
    clippy::type_complexity,
    reason = "the installation's commands, tokens and name, by value"
)]
pub(crate) fn installed<C, K, E, B>(
    news: News<C, K, E, B>,
) -> Result<(C, K, InstallationOf<B>), StationExitOf<E>> {
    match news {
        News::Installed(commands, tokens, installation) => Ok((commands, tokens, installation)),
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
/// (`None`: the stop's drain passed before it did), how the lifecycle's
/// half settles (only a release that succeeded retires it), and the actor
/// task's end as its news told, for a half that goes out. `failure` is the
/// lifecycle's own cause when it retired the actor after a refused start;
/// the exit keeps it.
pub(crate) fn release_settled<C, K, E: Copy, B>(
    news: Option<News<C, K, E, B>>,
    failure: Option<LifecycleFailure>,
) -> (StationExitOf<E>, LifecycleHalf, Option<LateEnd<E>>) {
    let told = news.map(ended);
    let exit = told.unwrap_or(StationExitOf::ReleaseFailed(ReleaseCause::Deadline));
    let half = match exit {
        StationExitOf::Stopped | StationExitOf::StoppedNetworkLost => LifecycleHalf::Retired,
        _ => LifecycleHalf::Unfinished,
    };
    (
        failure.map_or(exit, StationExitOf::Failed),
        half,
        told.map(LateEnd::Ended),
    )
}

/// The exit the actor task's last news gives.
pub(crate) fn ended<C, K, E, B>(news: News<C, K, E, B>) -> StationExitOf<E> {
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
