//! The radio's life: start, retune, stop, retire, release
//! (`design/port-contract-2026-11-16.md`, section 3).
//!
//! ```text
//!             start              a confirmed start        retune (the gate closed)
//!  Stopped ──────────► Starting ───────────────────► Running ◄──────────► Retuning
//!     ▲  ▲                 │ failed: the radio is stopped   │ stop            │ failed
//!     │  └─────────────────┴─────────────────────────────── │ ────────────────┘
//!     └──────────────── a confirmed stop ◄────────────── Stopping
//! ```
//!
//! Each position is a right: a value of no size, branded with the
//! installation (`'id`), neither `Copy` nor `Clone`. A call that needs a
//! position takes its right, and no call answers an error: a start or a
//! retune that fails answers [`Stopped`] beside its [`StartFailure`], and a
//! stop always ends in [`Stopped`].
//!
//! Only this crate builds a right. A port makes the next one from the one
//! it was given with its [`PortKey`], which the installation's mint gives
//! the port alone ([`PortPools`]); a core holds the rights the machine
//! holds and never the key, so it cannot write a transition the port did
//! not make.

use core::marker::PhantomData;

use oer_protocols_radio_port::{PoolBuilder, PoolPath, PoolSet, Root};

use crate::{GateClosed, GateIssuer, GateLent, GatePool};

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

macro_rules! right {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        #[derive(Debug)]
        pub struct $name<'id> {
            brand: Brand<'id>,
        }
    };
}

right!(
    /// The radio is stopped: what an install, a confirmed stop and a failed
    /// start or retune give, and what a start and a retirement take.
    Stopped
);
right!(
    /// The right to stop the radio, held by the installation machine from
    /// the install to the release.
    StopRight
);
right!(
    /// A start in progress.
    Starting
);
right!(
    /// The radio runs: a confirmed start or retune.
    Running
);
right!(
    /// A retune in progress.
    Retuning
);
right!(
    /// A stop in progress. It always ends in [`Stopped`].
    Stopping
);
right!(
    /// The units the network held are home: what a retirement takes.
    UnitsHome
);
right!(
    /// The installation is retired: what a release takes.
    Retired
);

/// Where a stop begins: a running radio, a start or a retune in progress.
#[must_use = "a stop begins from it"]
#[derive(Debug)]
pub enum StopFrom<'id> {
    /// The radio runs.
    Running(Running<'id>),
    /// A start is in progress.
    Starting(Starting<'id>),
    /// A retune is in progress.
    Retuning(Retuning<'id>),
}

/// The port's authority to make the next right from the one it was given.
/// The installation's mint gives it to the port alone; it is not `Copy`
/// nor `Clone`, and no core holds one.
#[must_use = "the port keeps its key for the whole installation"]
#[derive(Debug)]
pub struct PortKey<'id> {
    brand: Brand<'id>,
}

impl<'id> PortKey<'id> {
    /// A start began.
    pub const fn start(&self, stopped: Stopped<'id>) -> Starting<'id> {
        let Stopped { brand } = stopped;
        Starting { brand }
    }

    /// A start is confirmed: the radio runs.
    pub const fn started(&self, starting: Starting<'id>) -> Running<'id> {
        let Starting { brand } = starting;
        Running { brand }
    }

    /// A start failed: the port stopped what it began, and the radio is
    /// stopped.
    pub const fn start_failed(&self, starting: Starting<'id>) -> Stopped<'id> {
        let Starting { brand } = starting;
        Stopped { brand }
    }

    /// A retune began, under the closed gate.
    pub const fn retune(&self, running: Running<'id>) -> Retuning<'id> {
        let Running { brand } = running;
        Retuning { brand }
    }

    /// A retune is confirmed: the radio runs on the new channel.
    pub const fn retuned(&self, retuning: Retuning<'id>) -> Running<'id> {
        let Retuning { brand } = retuning;
        Running { brand }
    }

    /// A retune failed: the radio is stopped.
    pub const fn retune_failed(&self, retuning: Retuning<'id>) -> Stopped<'id> {
        let Retuning { brand } = retuning;
        Stopped { brand }
    }

    /// A stop began.
    pub const fn stop(&self, from: StopFrom<'id>) -> Stopping<'id> {
        let (StopFrom::Running(Running { brand })
        | StopFrom::Starting(Starting { brand })
        | StopFrom::Retuning(Retuning { brand })) = from;
        Stopping { brand }
    }

    /// A stop is confirmed.
    pub const fn stopped(&self, stopping: Stopping<'id>) -> Stopped<'id> {
        let Stopping { brand } = stopping;
        Stopped { brand }
    }

    /// The network returned every unit it held.
    pub const fn units_home(&self) -> UnitsHome<'id> {
        UnitsHome { brand: PhantomData }
    }

    /// The installation is retired.
    pub const fn retire(&self, stopped: Stopped<'id>, units: UnitsHome<'id>) -> Retired<'id> {
        let Stopped { brand } = stopped;
        let UnitsHome { brand: _ } = units;
        Retired { brand }
    }

    /// The installation is released: the retirement's and the stop's
    /// rights end here.
    pub const fn release(&self, retired: Retired<'id>, right: StopRight<'id>) {
        let Retired { brand: _ } = retired;
        let StopRight { brand: _ } = right;
    }
}

/// A port's key, the radio stopped and the stop's right, made without an
/// installation's mint: for the tests of a core that holds rights, never a
/// product build.
#[cfg(feature = "test-support")]
pub const fn forge<'id>() -> (PortKey<'id>, Stopped<'id>, StopRight<'id>) {
    (
        PortKey { brand: PhantomData },
        Stopped { brand: PhantomData },
        StopRight { brand: PhantomData },
    )
}

/// Why a start or a retune did not make the radio run. The port stopped
/// what it began before it answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartFailure {
    /// The PHY's step failed: the channel was not tuned.
    Phy,
    /// The MAC did not become ready (its cold handshake).
    MacReadiness,
    /// The start did not end within its bound.
    Bound,
}

/// A start's poll.
#[derive(Debug)]
pub enum StartPoll<'id> {
    /// Still in progress; the port's facts wake.
    Pending(Starting<'id>),
    /// The radio runs.
    Running(Running<'id>),
    /// The start failed and the radio is stopped.
    Stopped(Stopped<'id>, StartFailure),
}

/// A retune's poll.
#[derive(Debug)]
pub enum RetunePoll<'id> {
    /// Still in progress; the port's facts wake.
    Pending(Retuning<'id>),
    /// The radio runs on the new channel.
    Running(Running<'id>),
    /// The retune failed and the radio is stopped.
    Stopped(Stopped<'id>, StartFailure),
}

/// A stop's poll. A stop always ends.
#[derive(Debug)]
pub enum StopPoll<'id> {
    /// Still in progress; the port's facts wake.
    Pending(Stopping<'id>),
    /// The radio is stopped.
    Stopped(Stopped<'id>),
}

/// What an installation's mint gives: the port's key, the radio stopped,
/// the stop's right, the data's rights and the closed gate with its
/// issuer. The port keeps the key and the issuer; the machine takes the
/// rest, and the gate goes to the radio's controller.
#[must_use = "an installation's rights each have one holder"]
pub struct Install<'id, D, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    /// The port's key.
    pub key: PortKey<'id>,
    /// The radio is stopped.
    pub stopped: Stopped<'id>,
    /// The right to stop the radio.
    pub right: StopRight<'id>,
    /// The data's rights.
    pub data: D,
    /// The gate, closed: the data is stopped.
    pub gate: GateClosed<'id, Path>,
    /// The port's authority over the gate.
    pub issuer: GateIssuer<'id, Path>,
}

/// The pool set of an installation whose data's rights `D` builds.
pub struct PortPools<D>(PhantomData<fn() -> D>);

/// The data's rights an installation's mint builds. Only this crate
/// implements it ([`DataLayout`](crate::DataLayout)) and only its
/// installation's mint calls it: `mint` takes a seal no other crate can
/// make.
pub trait MintData: sealed::Layout {
    /// The rights, branded with the installation.
    type Rights<'id>;
    /// Build them, for the installation's mint alone.
    fn mint<'id>(seal: MintSeal) -> Self::Rights<'id>;
}

/// The seal of [`MintData::mint`]: only this crate makes one.
#[derive(Debug)]
pub struct MintSeal(pub(crate) ());

pub(crate) mod sealed {
    /// The data layouts of this crate.
    pub trait Layout {}
}

impl<D: MintData> PoolSet for PortPools<D> {
    type Pools<'id, Path: PoolPath> = Install<'id, D::Rights<'id>, Path>;

    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        let (gate, issuer) = builder.into_pools::<GatePool>();
        Install {
            key: PortKey { brand: PhantomData },
            stopped: Stopped { brand: PhantomData },
            right: StopRight { brand: PhantomData },
            data: D::mint(MintSeal(())),
            gate,
            issuer,
        }
    }
}

/// The radio's life, as a port gives it (section 3).
pub trait RadioPort<'id> {
    /// The port's capabilities.
    type Caps: crate::Ieee80211Capabilities;
    /// The channel a start or a retune is given.
    type Channel: Copy;
    /// The port's data's rights.
    type Data;
    /// What a release gives back: the hardware the installation held.
    type Released<'a>
    where
        Self: 'a;

    /// Begin a start of the stopped radio on `channel`.
    fn start(
        &mut self,
        stopped: Stopped<'id>,
        channel: Self::Channel,
        now: oer_portable_time::Instant,
    ) -> Starting<'id>;

    /// Poll a start.
    fn poll_start(
        &mut self,
        starting: Starting<'id>,
        cx: &mut core::task::Context<'_>,
        now: oer_portable_time::Instant,
    ) -> StartPoll<'id>;

    /// Begin a retune of the running radio to `channel`, under the
    /// installation's closed gate.
    fn retune(
        &mut self,
        running: Running<'id>,
        channel: Self::Channel,
        gate: GateLent<'_, 'id>,
        now: oer_portable_time::Instant,
    ) -> Retuning<'id>;

    /// Poll a retune.
    fn poll_retune(
        &mut self,
        retuning: Retuning<'id>,
        cx: &mut core::task::Context<'_>,
        now: oer_portable_time::Instant,
    ) -> RetunePoll<'id>;

    /// Begin the radio's stop from a running radio, a start or a retune.
    fn stop(&mut self, right: &mut StopRight<'id>, from: StopFrom<'id>) -> Stopping<'id>;

    /// Poll a stop. It always ends in [`Stopped`].
    fn poll_stop(
        &mut self,
        stopping: Stopping<'id>,
        cx: &mut core::task::Context<'_>,
        now: oer_portable_time::Instant,
    ) -> StopPoll<'id>;

    /// Whether every unit the network held is home.
    fn poll_units_home(
        &mut self,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<UnitsHome<'id>>;

    /// Retire the stopped installation, with its data's rights and its
    /// units home. It cannot refuse.
    fn retire(
        &mut self,
        stopped: Stopped<'id>,
        data: Self::Data,
        units: UnitsHome<'id>,
    ) -> Retired<'id>;

    /// Release the retired installation, with the stop's right.
    fn release(&mut self, retired: Retired<'id>, right: StopRight<'id>) -> Self::Released<'_>;
}
